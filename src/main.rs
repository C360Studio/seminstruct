use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use prometheus::{Counter, Encoder, Histogram, HistogramOpts, Opts, Registry, TextEncoder};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::net::TcpListener;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// ============================================================================
// Configuration
// ============================================================================

#[derive(Debug, Clone)]
pub struct Config {
    pub shimmy_url: String,
    pub port: u16,
    pub timeout_seconds: u64,
    pub max_retries: u32,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            shimmy_url: std::env::var("SEMINSTRUCT_SHIMMY_URL")
                .unwrap_or_else(|_| "http://localhost:8080".to_string()),
            port: std::env::var("SEMINSTRUCT_PORT")
                .unwrap_or_else(|_| "8083".to_string())
                .parse()
                .unwrap_or(8083),
            timeout_seconds: std::env::var("SEMINSTRUCT_TIMEOUT_SECONDS")
                .unwrap_or_else(|_| "120".to_string())
                .parse()
                .unwrap_or(120),
            max_retries: std::env::var("SEMINSTRUCT_MAX_RETRIES")
                .unwrap_or_else(|_| "3".to_string())
                .parse()
                .unwrap_or(3),
        }
    }
}

// ============================================================================
// OpenAI-Compatible Request/Response Types
// ============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Usage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelsResponse {
    pub object: String,
    pub data: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub owned_by: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub shimmy_url: String,
    pub shimmy_healthy: bool,
}

// ============================================================================
// Shimmy Client
// ============================================================================

#[derive(Debug, Error)]
pub enum ShimmyError {
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Shimmy returned error: {status} - {message}")]
    ShimmyError { status: u16, message: String },
    #[error("Shimmy is unavailable")]
    Unavailable,
}

pub struct ShimmyClient {
    client: Client,
    base_url: String,
    max_retries: u32,
}

impl ShimmyClient {
    pub fn new(base_url: String, timeout: Duration, max_retries: u32) -> Self {
        let client = Client::builder()
            .timeout(timeout)
            .pool_max_idle_per_host(10)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            base_url,
            max_retries,
        }
    }

    pub async fn chat_completions(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ShimmyError> {
        let url = format!("{}/v1/chat/completions", self.base_url);

        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let backoff = Duration::from_millis(100 * (1 << attempt));
                tokio::time::sleep(backoff).await;
                warn!("Retrying shimmy request (attempt {})", attempt + 1);
            }

            match self.client.post(&url).json(req).send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        return response
                            .json::<ChatCompletionResponse>()
                            .await
                            .map_err(ShimmyError::from);
                    } else {
                        let status = response.status().as_u16();
                        let message = response
                            .text()
                            .await
                            .unwrap_or_else(|_| "Unknown error".to_string());
                        last_error = Some(ShimmyError::ShimmyError { status, message });
                    }
                }
                Err(e) => {
                    last_error = Some(ShimmyError::Request(e));
                }
            }
        }

        Err(last_error.unwrap_or(ShimmyError::Unavailable))
    }

    pub async fn models(&self) -> Result<ModelsResponse, ShimmyError> {
        let url = format!("{}/v1/models", self.base_url);

        let response = self.client.get(&url).send().await?;

        if response.status().is_success() {
            response
                .json::<ModelsResponse>()
                .await
                .map_err(ShimmyError::from)
        } else {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(ShimmyError::ShimmyError { status, message })
        }
    }

    pub async fn health(&self) -> bool {
        let url = format!("{}/health", self.base_url);
        match self.client.get(&url).send().await {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }
}

// ============================================================================
// Application State
// ============================================================================

pub struct AppState {
    shimmy: ShimmyClient,
    config: Config,
    metrics: Arc<Metrics>,
}

// ============================================================================
// Metrics
// ============================================================================

pub struct Metrics {
    registry: Registry,
    requests_total: Counter,
    request_duration: Histogram,
    errors_total: Counter,
    shimmy_errors: Counter,
}

impl Metrics {
    pub fn new() -> anyhow::Result<Self> {
        let registry = Registry::new();

        let requests_total = Counter::with_opts(Opts::new(
            "seminstruct_requests_total",
            "Total number of chat completion requests",
        ))?;
        registry.register(Box::new(requests_total.clone()))?;

        let request_duration = Histogram::with_opts(HistogramOpts::new(
            "seminstruct_request_duration_seconds",
            "Request duration in seconds",
        ))?;
        registry.register(Box::new(request_duration.clone()))?;

        let errors_total = Counter::with_opts(Opts::new(
            "seminstruct_errors_total",
            "Total number of errors",
        ))?;
        registry.register(Box::new(errors_total.clone()))?;

        let shimmy_errors = Counter::with_opts(Opts::new(
            "seminstruct_shimmy_errors_total",
            "Total number of shimmy backend errors",
        ))?;
        registry.register(Box::new(shimmy_errors.clone()))?;

        Ok(Self {
            registry,
            requests_total,
            request_duration,
            errors_total,
            shimmy_errors,
        })
    }
}

// ============================================================================
// HTTP Handlers
// ============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "seminstruct=info,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting seminstruct service (shimmy proxy)");

    // Load configuration
    let config = Config::from_env();
    info!("Configuration: shimmy_url={}, port={}", config.shimmy_url, config.port);

    // Create shimmy client
    let shimmy = ShimmyClient::new(
        config.shimmy_url.clone(),
        Duration::from_secs(config.timeout_seconds),
        config.max_retries,
    );

    // Check shimmy health on startup
    if shimmy.health().await {
        info!("Shimmy backend is healthy");
    } else {
        warn!("Shimmy backend is not responding - will retry on requests");
    }

    // Initialize metrics
    let metrics = Arc::new(Metrics::new()?);

    // Create state
    let state = Arc::new(AppState {
        shimmy,
        config: config.clone(),
        metrics,
    });

    // Build router with OpenAI-compatible endpoints
    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions_handler))
        .route("/v1/models", get(models_handler))
        .route("/health", get(health_check))
        .route("/metrics", get(metrics_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Start server
    let addr = format!("0.0.0.0:{}", config.port);
    info!("Listening on {}", addr);
    info!("Proxying to shimmy at {}", config.shimmy_url);

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// OpenAI-compatible chat completions endpoint (proxied to shimmy)
async fn chat_completions_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<Json<ChatCompletionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let timer = state.metrics.request_duration.start_timer();
    state.metrics.requests_total.inc();

    // Validate request
    if req.messages.is_empty() {
        state.metrics.errors_total.inc();
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: ErrorDetail {
                    message: "Messages array cannot be empty".to_string(),
                    error_type: "invalid_request_error".to_string(),
                    code: Some("invalid_messages".to_string()),
                },
            }),
        ));
    }

    // Proxy request to shimmy
    match state.shimmy.chat_completions(&req).await {
        Ok(response) => {
            timer.observe_duration();
            Ok(Json(response))
        }
        Err(e) => {
            error!("Shimmy request failed: {}", e);
            state.metrics.errors_total.inc();
            state.metrics.shimmy_errors.inc();

            let (status, message) = match &e {
                ShimmyError::ShimmyError { status, message } => {
                    (StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY), message.clone())
                }
                ShimmyError::Unavailable => {
                    (StatusCode::SERVICE_UNAVAILABLE, "Shimmy backend is unavailable".to_string())
                }
                ShimmyError::Request(req_err) => {
                    if req_err.is_timeout() {
                        (StatusCode::GATEWAY_TIMEOUT, "Request to shimmy timed out".to_string())
                    } else {
                        (StatusCode::BAD_GATEWAY, format!("Shimmy request failed: {}", req_err))
                    }
                }
            };

            Err((
                status,
                Json(ErrorResponse {
                    error: ErrorDetail {
                        message,
                        error_type: "backend_error".to_string(),
                        code: Some("shimmy_error".to_string()),
                    },
                }),
            ))
        }
    }
}

/// List available models (proxied from shimmy)
async fn models_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.shimmy.models().await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(e) => {
            error!("Failed to get models from shimmy: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: ErrorDetail {
                        message: "Failed to get models from shimmy".to_string(),
                        error_type: "backend_error".to_string(),
                        code: Some("shimmy_error".to_string()),
                    },
                }),
            )
                .into_response()
        }
    }
}

async fn health_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let shimmy_healthy = state.shimmy.health().await;

    let status = if shimmy_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(HealthResponse {
            status: if shimmy_healthy { "healthy" } else { "degraded" }.to_string(),
            shimmy_url: state.config.shimmy_url.clone(),
            shimmy_healthy,
        }),
    )
}

async fn metrics_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = state.metrics.registry.gather();

    let mut buffer = Vec::new();
    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        error!("Failed to encode metrics: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to encode metrics".to_string(),
        );
    }

    match String::from_utf8(buffer) {
        Ok(metrics) => (StatusCode::OK, metrics),
        Err(e) => {
            error!("Failed to convert metrics to string: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to convert metrics".to_string(),
            )
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // Config Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_config_defaults() {
        // Clear any existing env vars
        std::env::remove_var("SEMINSTRUCT_SHIMMY_URL");
        std::env::remove_var("SEMINSTRUCT_PORT");
        std::env::remove_var("SEMINSTRUCT_TIMEOUT_SECONDS");
        std::env::remove_var("SEMINSTRUCT_MAX_RETRIES");

        let config = Config::from_env();

        assert_eq!(config.shimmy_url, "http://localhost:8080");
        assert_eq!(config.port, 8083);
        assert_eq!(config.timeout_seconds, 120);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_config_from_env() {
        std::env::set_var("SEMINSTRUCT_SHIMMY_URL", "http://shimmy:9000");
        std::env::set_var("SEMINSTRUCT_PORT", "9999");
        std::env::set_var("SEMINSTRUCT_TIMEOUT_SECONDS", "60");
        std::env::set_var("SEMINSTRUCT_MAX_RETRIES", "5");

        let config = Config::from_env();

        assert_eq!(config.shimmy_url, "http://shimmy:9000");
        assert_eq!(config.port, 9999);
        assert_eq!(config.timeout_seconds, 60);
        assert_eq!(config.max_retries, 5);

        // Cleanup
        std::env::remove_var("SEMINSTRUCT_SHIMMY_URL");
        std::env::remove_var("SEMINSTRUCT_PORT");
        std::env::remove_var("SEMINSTRUCT_TIMEOUT_SECONDS");
        std::env::remove_var("SEMINSTRUCT_MAX_RETRIES");
    }

    #[test]
    fn test_config_invalid_port_uses_default() {
        std::env::set_var("SEMINSTRUCT_PORT", "not_a_number");

        let config = Config::from_env();

        assert_eq!(config.port, 8083);

        std::env::remove_var("SEMINSTRUCT_PORT");
    }

    #[test]
    fn test_config_invalid_timeout_uses_default() {
        std::env::set_var("SEMINSTRUCT_TIMEOUT_SECONDS", "invalid");

        let config = Config::from_env();

        assert_eq!(config.timeout_seconds, 120);

        std::env::remove_var("SEMINSTRUCT_TIMEOUT_SECONDS");
    }

    // ------------------------------------------------------------------------
    // ShimmyError Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_shimmy_error_display_shimmy_error() {
        let err = ShimmyError::ShimmyError {
            status: 500,
            message: "Internal server error".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Shimmy returned error: 500 - Internal server error"
        );
    }

    #[test]
    fn test_shimmy_error_display_unavailable() {
        let err = ShimmyError::Unavailable;
        assert_eq!(err.to_string(), "Shimmy is unavailable");
    }

    // ------------------------------------------------------------------------
    // Serialization Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_chat_completion_request_serialization() {
        let req = ChatCompletionRequest {
            model: "mistral-7b-instruct".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "Hello!".to_string(),
            }],
            max_tokens: Some(100),
            temperature: Some(0.7),
            top_p: None,
            stream: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: ChatCompletionRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.model, "mistral-7b-instruct");
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, "user");
        assert_eq!(parsed.messages[0].content, "Hello!");
        assert_eq!(parsed.max_tokens, Some(100));
        assert_eq!(parsed.temperature, Some(0.7));
    }

    #[test]
    fn test_chat_completion_request_deserialization_minimal() {
        let json = r#"{
            "model": "gpt-3.5-turbo",
            "messages": [{"role": "user", "content": "Hi"}]
        }"#;

        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();

        assert_eq!(req.model, "gpt-3.5-turbo");
        assert_eq!(req.messages.len(), 1);
        assert!(req.max_tokens.is_none());
        assert!(req.temperature.is_none());
        assert!(req.stream.is_none());
    }

    #[test]
    fn test_chat_completion_response_serialization() {
        let resp = ChatCompletionResponse {
            id: "chatcmpl-123".to_string(),
            object: "chat.completion".to_string(),
            created: 1699000000,
            model: "mistral-7b-instruct".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: "Hello! How can I help?".to_string(),
                },
                finish_reason: "stop".to_string(),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            },
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("chatcmpl-123"));
        assert!(json.contains("chat.completion"));
        assert!(json.contains("Hello! How can I help?"));
    }

    #[test]
    fn test_health_response_serialization() {
        let resp = HealthResponse {
            status: "healthy".to_string(),
            shimmy_url: "http://shimmy:8080".to_string(),
            shimmy_healthy: true,
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"status\":\"healthy\""));
        assert!(json.contains("\"shimmy_healthy\":true"));
    }

    #[test]
    fn test_error_response_serialization() {
        let resp = ErrorResponse {
            error: ErrorDetail {
                message: "Something went wrong".to_string(),
                error_type: "server_error".to_string(),
                code: Some("internal_error".to_string()),
            },
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"message\":\"Something went wrong\""));
        assert!(json.contains("\"type\":\"server_error\""));
        assert!(json.contains("\"code\":\"internal_error\""));
    }

    #[test]
    fn test_error_response_without_code() {
        let resp = ErrorResponse {
            error: ErrorDetail {
                message: "Error occurred".to_string(),
                error_type: "invalid_request".to_string(),
                code: None,
            },
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("code"));
    }

    #[test]
    fn test_models_response_deserialization() {
        let json = r#"{
            "object": "list",
            "data": [
                {
                    "id": "mistral-7b-instruct",
                    "object": "model",
                    "created": 1699000000,
                    "owned_by": "shimmy"
                }
            ]
        }"#;

        let resp: ModelsResponse = serde_json::from_str(json).unwrap();

        assert_eq!(resp.object, "list");
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].id, "mistral-7b-instruct");
        assert_eq!(resp.data[0].owned_by, "shimmy");
    }

    // ------------------------------------------------------------------------
    // Metrics Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_metrics_initialization() {
        let metrics = Metrics::new().expect("Failed to create metrics");

        // Increment counters to verify they work
        metrics.requests_total.inc();
        metrics.errors_total.inc();
        metrics.shimmy_errors.inc();

        // Observe histogram
        let timer = metrics.request_duration.start_timer();
        timer.observe_duration();

        // Verify registry has metrics
        let families = metrics.registry.gather();
        assert!(!families.is_empty());
    }

    // ------------------------------------------------------------------------
    // ShimmyClient Tests (with mockito)
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn test_shimmy_client_health_success() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/health")
            .with_status(200)
            .with_body("OK")
            .create_async()
            .await;

        let client = ShimmyClient::new(
            server.url(),
            Duration::from_secs(5),
            3,
        );

        let healthy = client.health().await;
        assert!(healthy);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_shimmy_client_health_failure() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/health")
            .with_status(503)
            .create_async()
            .await;

        let client = ShimmyClient::new(
            server.url(),
            Duration::from_secs(5),
            3,
        );

        let healthy = client.health().await;
        assert!(!healthy);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_shimmy_client_health_connection_refused() {
        // Use an invalid URL that won't connect
        let client = ShimmyClient::new(
            "http://127.0.0.1:1".to_string(),
            Duration::from_millis(100),
            0,
        );

        let healthy = client.health().await;
        assert!(!healthy);
    }

    #[tokio::test]
    async fn test_shimmy_client_models_success() {
        let mut server = mockito::Server::new_async().await;

        let models_response = ModelsResponse {
            object: "list".to_string(),
            data: vec![ModelInfo {
                id: "mistral-7b-instruct".to_string(),
                object: "model".to_string(),
                created: 1699000000,
                owned_by: "shimmy".to_string(),
            }],
        };

        let mock = server
            .mock("GET", "/v1/models")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&models_response).unwrap())
            .create_async()
            .await;

        let client = ShimmyClient::new(
            server.url(),
            Duration::from_secs(5),
            3,
        );

        let result = client.models().await;
        assert!(result.is_ok());

        let models = result.unwrap();
        assert_eq!(models.data.len(), 1);
        assert_eq!(models.data[0].id, "mistral-7b-instruct");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_shimmy_client_models_error() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/models")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let client = ShimmyClient::new(
            server.url(),
            Duration::from_secs(5),
            3,
        );

        let result = client.models().await;
        assert!(result.is_err());

        match result.unwrap_err() {
            ShimmyError::ShimmyError { status, message } => {
                assert_eq!(status, 500);
                assert!(message.contains("Internal Server Error"));
            }
            _ => panic!("Expected ShimmyError"),
        }
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_shimmy_client_chat_completions_success() {
        let mut server = mockito::Server::new_async().await;

        let response = ChatCompletionResponse {
            id: "chatcmpl-test123".to_string(),
            object: "chat.completion".to_string(),
            created: 1699000000,
            model: "mistral-7b-instruct".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: "Hello! I'm here to help.".to_string(),
                },
                finish_reason: "stop".to_string(),
            }],
            usage: Usage {
                prompt_tokens: 5,
                completion_tokens: 10,
                total_tokens: 15,
            },
        };

        let mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&response).unwrap())
            .create_async()
            .await;

        let client = ShimmyClient::new(
            server.url(),
            Duration::from_secs(5),
            3,
        );

        let request = ChatCompletionRequest {
            model: "mistral-7b-instruct".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "Hello!".to_string(),
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: None,
        };

        let result = client.chat_completions(&request).await;
        assert!(result.is_ok());

        let resp = result.unwrap();
        assert_eq!(resp.id, "chatcmpl-test123");
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(resp.choices[0].message.content, "Hello! I'm here to help.");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_shimmy_client_chat_completions_error() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(400)
            .with_body("Bad Request: Invalid model")
            .create_async()
            .await;

        let client = ShimmyClient::new(
            server.url(),
            Duration::from_secs(5),
            0, // No retries for faster test
        );

        let request = ChatCompletionRequest {
            model: "invalid-model".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "Hello!".to_string(),
            }],
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: None,
        };

        let result = client.chat_completions(&request).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            ShimmyError::ShimmyError { status, message } => {
                assert_eq!(status, 400);
                assert!(message.contains("Invalid model"));
            }
            _ => panic!("Expected ShimmyError"),
        }
        mock.assert_async().await;
    }

    // ------------------------------------------------------------------------
    // Request Validation Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_empty_messages_detected() {
        let req = ChatCompletionRequest {
            model: "mistral-7b-instruct".to_string(),
            messages: vec![], // Empty!
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: None,
        };

        assert!(req.messages.is_empty());
    }

    // ------------------------------------------------------------------------
    // ShimmyClient Unit Tests (without HTTP mocking)
    // ------------------------------------------------------------------------

    #[test]
    fn test_shimmy_client_creation() {
        let client = ShimmyClient::new(
            "http://localhost:8080".to_string(),
            Duration::from_secs(30),
            5,
        );

        assert_eq!(client.base_url, "http://localhost:8080");
        assert_eq!(client.max_retries, 5);
    }
}
