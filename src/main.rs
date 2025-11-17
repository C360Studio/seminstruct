use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::llama::{Cache, Llama, LlamaConfig};
use hf_hub::api::sync::Api;
use prometheus::{Counter, Encoder, Histogram, HistogramOpts, Opts, Registry, TextEncoder};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokenizers::Tokenizer;
use tokio::net::TcpListener;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// Request/Response types
#[derive(Debug, Deserialize)]
struct SummarizeRequest {
    text: String,
    #[serde(default = "default_max_length")]
    max_length: usize,
    #[serde(default = "default_min_length")]
    min_length: usize,
}

fn default_max_length() -> usize {
    100
}

fn default_min_length() -> usize {
    20
}

#[derive(Debug, Serialize)]
struct SummarizeResponse {
    summary: String,
    model: String,
    latency_ms: f64,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: ErrorDetail,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    message: String,
    #[serde(rename = "type")]
    error_type: String,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: String,
    model: String,
}

// Application state
struct AppState {
    model: Mutex<ModelState>,
    model_name: String,
    metrics: Arc<Metrics>,
}

struct ModelState {
    llama: Llama,
    tokenizer: Tokenizer,
    cache: Cache,
    device: Device,
    config: candle_transformers::models::llama::Config,
    dtype: DType,
}

impl ModelState {
    fn load(model_id: &str) -> anyhow::Result<Self> {
        info!("Loading model from HuggingFace: {}", model_id);

        let device = Device::Cpu;
        let dtype = DType::F32;

        // Download model
        let api = Api::new()?;
        let api_repo = api.model(model_id.to_string());

        info!("Downloading model files...");
        let config_path = api_repo.get("config.json")?;
        let tokenizer_path = api_repo.get("tokenizer.json")?;
        let weights_path = api_repo.get("model.safetensors")?;

        info!("Loading model into memory...");

        // Load config
        let config: LlamaConfig = serde_json::from_slice(&std::fs::read(config_path)?)?;
        let config = config.into_config(false); // no flash attention

        // Load weights
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[weights_path], dtype, &device)? };
        let llama = Llama::load(vb, &config)?;

        // Load tokenizer
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        // Create cache
        let cache = Cache::new(true, dtype, &config, &device)?;

        info!("Model loaded successfully");

        Ok(Self {
            llama,
            tokenizer,
            cache,
            device,
            config: config.clone(),
            dtype,
        })
    }

    fn generate(&mut self, prompt: &str, max_tokens: usize) -> anyhow::Result<String> {
        // Reset cache for new generation (prevents shape mismatches between requests)
        self.cache = Cache::new(true, self.dtype, &self.config, &self.device)?;

        // Tokenize
        let mut tokens = self
            .tokenizer
            .encode(prompt, true)
            .map_err(|e| anyhow::anyhow!("Encoding failed: {}", e))?
            .get_ids()
            .to_vec();

        // Setup generation with temperature for diversity
        let mut logits_processor = LogitsProcessor::from_sampling(
            299792458,
            Sampling::All { temperature: 0.7 }, // Some randomness for better quality
        );

        let eos_token = 2u32; // Standard EOS for Llama-family models
        let mut generated = Vec::new();

        // Generation loop (blue-collar simple approach)
        for index in 0..max_tokens {
            let context_size = if index > 0 { 1 } else { tokens.len() };

            let ctxt = &tokens[tokens.len().saturating_sub(context_size)..];
            let input = Tensor::new(ctxt, &self.device)?.unsqueeze(0)?;

            // start_pos is the position in the sequence where this input starts
            let start_pos = tokens.len() - context_size;
            let logits = self.llama.forward(&input, start_pos, &mut self.cache)?;
            let logits = logits.squeeze(0)?.squeeze(0)?;

            let next_token = logits_processor.sample(&logits)?;

            if next_token == eos_token {
                break;
            }

            tokens.push(next_token);
            generated.push(next_token);
        }

        // Decode
        let summary = self
            .tokenizer
            .decode(&generated, true)
            .map_err(|e| anyhow::anyhow!("Decoding failed: {}", e))?;

        Ok(summary.trim().to_string())
    }
}

// Prometheus metrics
struct Metrics {
    registry: Registry,
    requests_total: Counter,
    request_duration: Histogram,
    errors_total: Counter,
}

impl Metrics {
    fn new() -> anyhow::Result<Self> {
        let registry = Registry::new();

        let requests_total = Counter::with_opts(Opts::new(
            "semsummarize_requests_total",
            "Total number of summarization requests",
        ))?;
        registry.register(Box::new(requests_total.clone()))?;

        let request_duration = Histogram::with_opts(HistogramOpts::new(
            "semsummarize_request_duration_seconds",
            "Request duration in seconds",
        ))?;
        registry.register(Box::new(request_duration.clone()))?;

        let errors_total = Counter::with_opts(Opts::new(
            "semsummarize_errors_total",
            "Total number of errors",
        ))?;
        registry.register(Box::new(errors_total.clone()))?;

        Ok(Self {
            registry,
            requests_total,
            request_duration,
            errors_total,
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "semsummarize=info,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting semsummarize service");

    // Get config
    let model_name = std::env::var("SEMSUMMARIZE_MODEL")
        .unwrap_or_else(|_| "HuggingFaceTB/SmolLM2-135M-Instruct".to_string());
    let port = std::env::var("SEMSUMMARIZE_PORT")
        .unwrap_or_else(|_| "8083".to_string())
        .parse::<u16>()?;

    // Load model
    let model = ModelState::load(&model_name)?;

    // Initialize metrics
    let metrics = Arc::new(Metrics::new()?);

    // Create state
    let state = Arc::new(AppState {
        model: Mutex::new(model),
        model_name: model_name.clone(),
        metrics: metrics.clone(),
    });

    // Build router
    let app = Router::new()
        .route("/summarize", post(create_summary))
        .route("/health", get(health_check))
        .route("/metrics", get(metrics_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Start server
    let addr = format!("0.0.0.0:{}", port);
    info!("Listening on {}", addr);

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn create_summary(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SummarizeRequest>,
) -> Result<Json<SummarizeResponse>, (StatusCode, Json<ErrorResponse>)> {
    let start = std::time::Instant::now();
    let timer = state.metrics.request_duration.start_timer();
    state.metrics.requests_total.inc();

    // Validate input
    if req.text.is_empty() {
        state.metrics.errors_total.inc();
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: ErrorDetail {
                    message: "Input text cannot be empty".to_string(),
                    error_type: "invalid_request_error".to_string(),
                },
            }),
        ));
    }

    // Build prompt - keep it simple and direct
    let prompt = format!(
        "<|im_start|>user\nWrite a one-sentence summary:\n{}\n<|im_end|>\n<|im_start|>assistant\nThis community contains ",
        req.text.chars().take(400).collect::<String>() // Limit input, start the response
    );

    // Generate summary
    let summary = {
        let mut model = state.model.lock().unwrap();
        match model.generate(&prompt, req.max_length.min(100)) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Generation failed: {}", e);
                state.metrics.errors_total.inc();
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: ErrorDetail {
                            message: format!("Failed to generate summary: {}", e),
                            error_type: "internal_error".to_string(),
                        },
                    }),
                ));
            }
        }
    };

    let latency_ms = start.elapsed().as_millis() as f64;

    let response = SummarizeResponse {
        summary,
        model: state.model_name.clone(),
        latency_ms,
    };

    timer.observe_duration();
    Ok(Json(response))
}

async fn health_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(HealthResponse {
        status: "healthy".to_string(),
        model: state.model_name.clone(),
    })
}

async fn metrics_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = state.metrics.registry.gather();

    let mut buffer = Vec::new();
    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        tracing::error!("Failed to encode metrics: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to encode metrics".to_string(),
        );
    }

    match String::from_utf8(buffer) {
        Ok(metrics) => (StatusCode::OK, metrics),
        Err(e) => {
            tracing::error!("Failed to convert metrics to string: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to convert metrics".to_string(),
            )
        }
    }
}
