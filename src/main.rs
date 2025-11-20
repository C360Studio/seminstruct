use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::t5;
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
    model: t5::T5ForConditionalGeneration,
    tokenizer: Tokenizer,
    device: Device,
    config: t5::Config,
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

        // Try to get weights - handle both single file and sharded models
        let weights_paths = match api_repo.get("model.safetensors") {
            Ok(path) => vec![path],
            Err(_) => {
                // Try sharded model format
                info!("Single safetensors not found, trying sharded format...");
                let index_path = api_repo.get("model.safetensors.index.json")?;
                let index: serde_json::Value = serde_json::from_slice(&std::fs::read(&index_path)?)?;
                let weight_map = index["weight_map"].as_object()
                    .ok_or_else(|| anyhow::anyhow!("Invalid safetensors index"))?;

                let mut files = std::collections::HashSet::new();
                for file in weight_map.values() {
                    if let Some(filename) = file.as_str() {
                        files.insert(filename.to_string());
                    }
                }

                let mut paths = Vec::new();
                for file in files {
                    paths.push(api_repo.get(&file)?);
                }
                paths
            }
        };

        info!("Loading model into memory...");

        // Load config
        let config_str = std::fs::read_to_string(config_path)?;
        let mut config: t5::Config = serde_json::from_str(&config_str)?;
        config.use_cache = false; // Disable cache for debugging

        // Load weights
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&weights_paths, dtype, &device)?
        };
        let model = t5::T5ForConditionalGeneration::load(vb, &config)?;

        // Load tokenizer
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        info!("Model loaded successfully");

        Ok(Self {
            model,
            tokenizer,
            device,
            config,
            dtype,
        })
    }

    fn generate(&mut self, prompt: &str, max_tokens: usize) -> anyhow::Result<String> {
        // Tokenize input
        let tokens = self
            .tokenizer
            .encode(prompt, true)
            .map_err(|e| anyhow::anyhow!("Encoding failed: {}", e))?
            .get_ids()
            .to_vec();

        let input_token_ids = Tensor::new(&tokens[..], &self.device)?.unsqueeze(0)?;

        // Encode input using T5 encoder
        let encoder_output = self.model.encode(&input_token_ids)?;

        // Setup generation with low temperature for factual summaries
        let temperature = Some(0.1); // Low temperature for deterministic, factual outputs
        let top_p = None; // Disable nucleus sampling for consistency
        let mut logits_processor = LogitsProcessor::new(299792458, temperature, top_p);

        // Use EOS token from config
        let eos_token = self.config.eos_token_id as u32;

        // Start with decoder start token from config (defaults to 0 for T5)
        let decoder_start_token = self.config.decoder_start_token_id.unwrap_or(0) as u32;
        let mut output_token_ids = vec![decoder_start_token];

        // Decoder generation loop
        for index in 0..max_tokens {
            // For T5 with cache: only pass last token after first iteration
            let decoder_token_ids = if index == 0 || !self.config.use_cache {
                Tensor::new(output_token_ids.as_slice(), &self.device)?.unsqueeze(0)?
            } else {
                let last_token = *output_token_ids.last().unwrap();
                Tensor::new(&[last_token], &self.device)?.unsqueeze(0)?
            };

            // Decode and get logits
            let logits = self.model.decode(&decoder_token_ids, &encoder_output)?
                .squeeze(0)?;

            // Apply repeat penalty to prevent repetition
            let repeat_penalty = 1.1;
            let repeat_last_n = 64;
            let start_at = output_token_ids.len().saturating_sub(repeat_last_n);
            let logits = candle_transformers::utils::apply_repeat_penalty(
                &logits,
                repeat_penalty,
                &output_token_ids[start_at..],
            )?;

            let next_token = logits_processor.sample(&logits)?;

            if next_token == eos_token {
                break;
            }

            output_token_ids.push(next_token);
        }

        // Decode output (skip the start token)
        let output_tokens = if output_token_ids.len() > 1 {
            &output_token_ids[1..]
        } else {
            &[]
        };

        let summary = self
            .tokenizer
            .decode(output_tokens, true)
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
        .unwrap_or_else(|_| "google/flan-t5-small".to_string());
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

    // Build prompt - T5/Flan-T5 uses simple instruction format
    // Limit to first 400 words to avoid overly long inputs
    let text = req.text.split_whitespace().take(400).collect::<Vec<_>>().join(" ");
    let prompt = format!("summarize: {}", text);

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
