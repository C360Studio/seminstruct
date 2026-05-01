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
    pub backend_url: String,
    pub port: u16,
    pub timeout_seconds: u64,
    pub max_retries: u32,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            backend_url: std::env::var("SEMINSTRUCT_BACKEND_URL")
                .unwrap_or_else(|_| "http://localhost:11435".to_string()),
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
    pub backend_url: String,
    pub backend_healthy: bool,
}

#[derive(Debug, Serialize)]
pub struct ReadyResponse {
    pub ready: bool,
    pub backend_url: String,
}

// ============================================================================
// Backend Client
// ============================================================================

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Backend returned error: {status} - {message}")]
    Status { status: u16, message: String },
    #[error("Backend is unavailable")]
    Unavailable,
}

pub struct BackendClient {
    client: Client,
    base_url: String,
    max_retries: u32,
}

impl BackendClient {
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
    ) -> Result<ChatCompletionResponse, BackendError> {
        let url = format!("{}/v1/chat/completions", self.base_url);

        // Force non-streaming - proxy doesn't support streaming responses yet
        if req.stream == Some(true) {
            warn!("Client requested streaming but proxy doesn't support it - forcing non-streaming");
        }
        let mut request = req.clone();
        request.stream = Some(false);

        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let backoff = Duration::from_millis(100 * (1 << attempt));
                tokio::time::sleep(backoff).await;
                warn!("Retrying backend request (attempt {})", attempt + 1);
            }

            match self.client.post(&url).json(&request).send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        return response
                            .json::<ChatCompletionResponse>()
                            .await
                            .map_err(BackendError::from);
                    } else {
                        let status = response.status().as_u16();
                        let message = response
                            .text()
                            .await
                            .unwrap_or_else(|_| "Unknown error".to_string());
                        last_error = Some(BackendError::Status { status, message });
                    }
                }
                Err(e) => {
                    last_error = Some(BackendError::Request(e));
                }
            }
        }

        Err(last_error.unwrap_or(BackendError::Unavailable))
    }

    pub async fn models(&self) -> Result<ModelsResponse, BackendError> {
        let url = format!("{}/v1/models", self.base_url);

        let response = self.client.get(&url).send().await?;

        if response.status().is_success() {
            response
                .json::<ModelsResponse>()
                .await
                .map_err(BackendError::from)
        } else {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(BackendError::Status { status, message })
        }
    }

    pub async fn health(&self) -> bool {
        let url = format!("{}/health", self.base_url);
        match self.client.get(&url).send().await {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    /// Check if the backend is ready to serve inference requests.
    /// Performs a lightweight inference call to verify the model is loaded.
    pub async fn ready(&self) -> bool {
        // Use qwen alias for readiness check - the default GGUF baked into
        // semserve. llama-server is started with --alias qwen2.5-0.5b.
        let request = ChatCompletionRequest {
            model: "qwen2.5-0.5b".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
            }],
            max_tokens: Some(1),
            temperature: None,
            top_p: None,
            stream: Some(false),
        };

        match self.chat_completions(&request).await {
            Ok(_) => true,
            Err(e) => {
                warn!("Backend readiness check failed: {}", e);
                false
            }
        }
    }
}

// ============================================================================
// Application State
// ============================================================================

pub struct AppState {
    backend: BackendClient,
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
    backend_errors: Counter,
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

        let backend_errors = Counter::with_opts(Opts::new(
            "seminstruct_backend_errors_total",
            "Total number of inference backend errors",
        ))?;
        registry.register(Box::new(backend_errors.clone()))?;

        Ok(Self {
            registry,
            requests_total,
            request_duration,
            errors_total,
            backend_errors,
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

    info!("Starting seminstruct service (inference backend proxy)");

    // Load configuration
    let config = Config::from_env();
    info!("Configuration: backend_url={}, port={}", config.backend_url, config.port);

    // Create backend client
    let backend = BackendClient::new(
        config.backend_url.clone(),
        Duration::from_secs(config.timeout_seconds),
        config.max_retries,
    );

    // Check backend health on startup
    if backend.health().await {
        info!("Inference backend is healthy");
    } else {
        warn!("Inference backend is not responding - will retry on requests");
    }

    // Initialize metrics
    let metrics = Arc::new(Metrics::new()?);

    // Create state
    let state = Arc::new(AppState {
        backend,
        config: config.clone(),
        metrics,
    });

    // Build router with OpenAI-compatible endpoints
    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions_handler))
        .route("/v1/models", get(models_handler))
        .route("/health", get(health_check))
        .route("/ready", get(ready_check))
        .route("/metrics", get(metrics_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Start server
    let addr = format!("0.0.0.0:{}", config.port);
    info!("Listening on {}", addr);
    info!("Proxying to backend at {}", config.backend_url);

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// OpenAI-compatible chat completions endpoint (proxied to inference backend)
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

    // Proxy request to backend
    match state.backend.chat_completions(&req).await {
        Ok(response) => {
            timer.observe_duration();
            Ok(Json(response))
        }
        Err(e) => {
            error!("Backend request failed: {}", e);
            state.metrics.errors_total.inc();
            state.metrics.backend_errors.inc();

            let (status, message) = match &e {
                BackendError::Status { status, message } => {
                    (StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY), message.clone())
                }
                BackendError::Unavailable => {
                    (StatusCode::SERVICE_UNAVAILABLE, "Inference backend is unavailable".to_string())
                }
                BackendError::Request(req_err) => {
                    if req_err.is_timeout() {
                        (StatusCode::GATEWAY_TIMEOUT, "Request to backend timed out".to_string())
                    } else {
                        (StatusCode::BAD_GATEWAY, format!("Backend request failed: {}", req_err))
                    }
                }
            };

            Err((
                status,
                Json(ErrorResponse {
                    error: ErrorDetail {
                        message,
                        error_type: "backend_error".to_string(),
                        code: Some("backend_error".to_string()),
                    },
                }),
            ))
        }
    }
}

/// List available models (proxied from backend)
async fn models_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.backend.models().await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(e) => {
            error!("Failed to get models from backend: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: ErrorDetail {
                        message: "Failed to get models from backend".to_string(),
                        error_type: "backend_error".to_string(),
                        code: Some("backend_error".to_string()),
                    },
                }),
            )
                .into_response()
        }
    }
}

async fn health_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let backend_healthy = state.backend.health().await;

    // Always return 200 - proxy is healthy, backend status in body
    (
        StatusCode::OK,
        Json(HealthResponse {
            status: if backend_healthy { "healthy" } else { "degraded" }.to_string(),
            backend_url: state.config.backend_url.clone(),
            backend_healthy,
        }),
    )
}

/// Readiness check - returns 200 only when the backend can complete inference.
/// Use this for Kubernetes readiness probes.
async fn ready_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let ready = state.backend.ready().await;

    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(ReadyResponse {
            ready,
            backend_url: state.config.backend_url.clone(),
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
    use std::sync::Mutex;

    // Serializes tests that mutate process env vars. Cargo runs tests in
    // parallel by default, and SEMINSTRUCT_* vars set by one test would
    // leak into another's Config::from_env() read.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ------------------------------------------------------------------------
    // Config Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_config_defaults() {
        let _guard = env_guard();
        std::env::remove_var("SEMINSTRUCT_BACKEND_URL");
        std::env::remove_var("SEMINSTRUCT_PORT");
        std::env::remove_var("SEMINSTRUCT_TIMEOUT_SECONDS");
        std::env::remove_var("SEMINSTRUCT_MAX_RETRIES");

        let config = Config::from_env();

        assert_eq!(config.backend_url, "http://localhost:11435");
        assert_eq!(config.port, 8083);
        assert_eq!(config.timeout_seconds, 120);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_config_from_env() {
        let _guard = env_guard();
        std::env::set_var("SEMINSTRUCT_BACKEND_URL", "http://semserve:9000");
        std::env::set_var("SEMINSTRUCT_PORT", "9999");
        std::env::set_var("SEMINSTRUCT_TIMEOUT_SECONDS", "60");
        std::env::set_var("SEMINSTRUCT_MAX_RETRIES", "5");

        let config = Config::from_env();

        assert_eq!(config.backend_url, "http://semserve:9000");
        assert_eq!(config.port, 9999);
        assert_eq!(config.timeout_seconds, 60);
        assert_eq!(config.max_retries, 5);

        std::env::remove_var("SEMINSTRUCT_BACKEND_URL");
        std::env::remove_var("SEMINSTRUCT_PORT");
        std::env::remove_var("SEMINSTRUCT_TIMEOUT_SECONDS");
        std::env::remove_var("SEMINSTRUCT_MAX_RETRIES");
    }

    #[test]
    fn test_config_invalid_port_uses_default() {
        let _guard = env_guard();
        std::env::set_var("SEMINSTRUCT_PORT", "not_a_number");

        let config = Config::from_env();

        assert_eq!(config.port, 8083);

        std::env::remove_var("SEMINSTRUCT_PORT");
    }

    #[test]
    fn test_config_invalid_timeout_uses_default() {
        let _guard = env_guard();
        std::env::set_var("SEMINSTRUCT_TIMEOUT_SECONDS", "invalid");

        let config = Config::from_env();

        assert_eq!(config.timeout_seconds, 120);

        std::env::remove_var("SEMINSTRUCT_TIMEOUT_SECONDS");
    }

    // ------------------------------------------------------------------------
    // BackendError Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_backend_error_display_status() {
        let err = BackendError::Status {
            status: 500,
            message: "Internal server error".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Backend returned error: 500 - Internal server error"
        );
    }

    #[test]
    fn test_backend_error_display_unavailable() {
        let err = BackendError::Unavailable;
        assert_eq!(err.to_string(), "Backend is unavailable");
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
            backend_url: "http://semserve:11435".to_string(),
            backend_healthy: true,
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"status\":\"healthy\""));
        assert!(json.contains("\"backend_healthy\":true"));
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
                    "id": "qwen2.5-0.5b",
                    "object": "model",
                    "created": 1699000000,
                    "owned_by": "llama-server"
                }
            ]
        }"#;

        let resp: ModelsResponse = serde_json::from_str(json).unwrap();

        assert_eq!(resp.object, "list");
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].id, "qwen2.5-0.5b");
        assert_eq!(resp.data[0].owned_by, "llama-server");
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
        metrics.backend_errors.inc();

        // Observe histogram
        let timer = metrics.request_duration.start_timer();
        timer.observe_duration();

        // Verify registry has metrics
        let families = metrics.registry.gather();
        assert!(!families.is_empty());
    }

    // ------------------------------------------------------------------------
    // BackendClient Tests (with mockito)
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn test_backend_client_health_success() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/health")
            .with_status(200)
            .with_body("OK")
            .create_async()
            .await;

        let client = BackendClient::new(
            server.url(),
            Duration::from_secs(5),
            3,
        );

        let healthy = client.health().await;
        assert!(healthy);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_backend_client_health_failure() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/health")
            .with_status(503)
            .create_async()
            .await;

        let client = BackendClient::new(
            server.url(),
            Duration::from_secs(5),
            3,
        );

        let healthy = client.health().await;
        assert!(!healthy);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_backend_client_health_connection_refused() {
        // Use an invalid URL that won't connect
        let client = BackendClient::new(
            "http://127.0.0.1:1".to_string(),
            Duration::from_millis(100),
            0,
        );

        let healthy = client.health().await;
        assert!(!healthy);
    }

    #[tokio::test]
    async fn test_backend_client_models_success() {
        let mut server = mockito::Server::new_async().await;

        let models_response = ModelsResponse {
            object: "list".to_string(),
            data: vec![ModelInfo {
                id: "qwen2.5-0.5b".to_string(),
                object: "model".to_string(),
                created: 1699000000,
                owned_by: "llama-server".to_string(),
            }],
        };

        let mock = server
            .mock("GET", "/v1/models")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&models_response).unwrap())
            .create_async()
            .await;

        let client = BackendClient::new(
            server.url(),
            Duration::from_secs(5),
            3,
        );

        let result = client.models().await;
        assert!(result.is_ok());

        let models = result.unwrap();
        assert_eq!(models.data.len(), 1);
        assert_eq!(models.data[0].id, "qwen2.5-0.5b");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_backend_client_models_error() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/models")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let client = BackendClient::new(
            server.url(),
            Duration::from_secs(5),
            3,
        );

        let result = client.models().await;
        assert!(result.is_err());

        match result.unwrap_err() {
            BackendError::Status { status, message } => {
                assert_eq!(status, 500);
                assert!(message.contains("Internal Server Error"));
            }
            _ => panic!("Expected BackendError::Status"),
        }
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_backend_client_chat_completions_success() {
        let mut server = mockito::Server::new_async().await;

        let response = ChatCompletionResponse {
            id: "chatcmpl-test123".to_string(),
            object: "chat.completion".to_string(),
            created: 1699000000,
            model: "qwen2.5-0.5b".to_string(),
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

        let client = BackendClient::new(
            server.url(),
            Duration::from_secs(5),
            3,
        );

        let request = ChatCompletionRequest {
            model: "qwen2.5-0.5b".to_string(),
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
    async fn test_backend_client_chat_completions_error() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(400)
            .with_body("Bad Request: Invalid model")
            .create_async()
            .await;

        let client = BackendClient::new(
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
            BackendError::Status { status, message } => {
                assert_eq!(status, 400);
                assert!(message.contains("Invalid model"));
            }
            _ => panic!("Expected BackendError::Status"),
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
    // BackendClient Unit Tests (without HTTP mocking)
    // ------------------------------------------------------------------------

    #[test]
    fn test_backend_client_creation() {
        let client = BackendClient::new(
            "http://localhost:11435".to_string(),
            Duration::from_secs(30),
            5,
        );

        assert_eq!(client.base_url, "http://localhost:11435");
        assert_eq!(client.max_retries, 5);
    }
}
