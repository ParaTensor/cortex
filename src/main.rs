use std::net::SocketAddr;
use std::sync::Arc;
use axum::{
    routing::{delete, get, post},
    Router,
};
use clap::Parser;
use dashmap::DashMap;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

use cortex::config::CortexConfig;
use cortex::hasher::{TokenizerEngine, TokenizerRegistry};
use cortex::ledger::{RadixHashTree, WorkerRuntimeState};
use cortex::metrics::{health_live, health_ready};
use cortex::proxy::{
    chat_completions_handler, cluster_status_handler, list_models_handler, session_close_handler,
    session_publish_handler, AppState,
};
use cortex::session_ledger::SessionLedger;
use cortex::scheduler::LocalityScheduler;

/// Resolves the effective chat template for a tokenizer path.
///
/// Alignment contract (docs/tokenizer-hash-alignment.md): the gateway MUST hash
/// the exact same token sequence the engine will see. Engines render prompts
/// with the model's Jinja chat template, so when the operator has not pinned an
/// explicit template we auto-discover `chat_template` from the sibling
/// `tokenizer_config.json` instead of falling back to naive concatenation
/// (which would desynchronize page-0 hashes and permanently break exact KV matching).
fn resolve_chat_template(tok_path: &str, configured: Option<String>) -> Option<String> {
    if configured.is_some() {
        return configured;
    }
    let cfg_path = std::path::Path::new(tok_path)
        .parent()?
        .join("tokenizer_config.json");
    let raw = std::fs::read_to_string(&cfg_path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("chat_template")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

#[derive(Parser, Debug)]
#[command(name = "cortex", about = "High-performance cluster KV-Cache aware & PD disaggregation inference gateway")]
struct Args {
    #[arg(short, long, default_value = "cortex.yaml")]
    config: String,
    #[arg(long, default_value = "0.0.0.0")]
    host: String,
    #[arg(long, default_value = "8000")]
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cortex=info,tower_http=debug".into()),
        )
        .init();

    let args = Args::parse();
    info!(
        version = env!("CARGO_PKG_VERSION"),
        "Starting Cortex Cluster Inference Gateway"
    );

    // Load config if exists, otherwise fallback to defaults
    let config = if std::path::Path::new(&args.config).exists() {
        let content = std::fs::read_to_string(&args.config)?;
        serde_yaml::from_str::<CortexConfig>(&content)?
    } else {
        info!("Config file '{}' not found, using default configuration", args.config);
        CortexConfig {
            host: args.host,
            port: args.port,
            ..Default::default()
        }
    };

    let tree = Arc::new(RadixHashTree::new());
    let workers = Arc::new(DashMap::new());

    // Register initial workers from config
    for w_cfg in &config.workers {
        info!(worker_id = %w_cfg.id, model = %w_cfg.model, endpoint = %w_cfg.http_endpoint, "Registering worker");
        let worker_state = Arc::new(WorkerRuntimeState::new(w_cfg.clone()));
        workers.insert(w_cfg.id.clone(), worker_state);
    }

    let kv_event_processor = Arc::new(cortex::zmq::KvEventProcessor::new(tree.clone()));
    cortex::zmq::spawn_all_worker_zmq_subscribers(&workers, kv_event_processor);

    let scheduler = Arc::new(LocalityScheduler::new(
        config.scheduler.clone(),
        tree.clone(),
        workers.clone(),
    ));

    // Initialize Tokenizer Registry and register configured models
    let tokenizer_registry = Arc::new(TokenizerRegistry::new(10000));

    for model_cfg in &config.models {
        if let Some(tok_path) = &model_cfg.tokenizer_path {
            let chat_template =
                resolve_chat_template(tok_path, model_cfg.chat_template.clone());
            if model_cfg.chat_template.is_none() && chat_template.is_some() {
                info!(model_id = %model_cfg.model_id, "Discovered chat template from tokenizer_config.json");
            }
            match TokenizerEngine::from_file(tok_path, chat_template) {
                Ok(engine) => {
                    info!(model_id = %model_cfg.model_id, path = %tok_path, "Successfully loaded and registered tokenizer");
                    tokenizer_registry.register(&model_cfg.model_id, engine);
                }
                Err(e) => {
                    warn!(model_id = %model_cfg.model_id, path = %tok_path, error = %e, "Failed to load tokenizer; will fallback to load-aware routing");
                }
            }
        }
    }

    // Also check worker configs for per-worker tokenizer paths
    for w_cfg in &config.workers {
        if let Some(tok_path) = &w_cfg.tokenizer_path {
            if !tokenizer_registry.contains_model(&w_cfg.model) {
                let chat_template = resolve_chat_template(tok_path, None);
                match TokenizerEngine::from_file(tok_path, chat_template) {
                    Ok(engine) => {
                        info!(model_id = %w_cfg.model, path = %tok_path, "Successfully loaded worker tokenizer");
                        tokenizer_registry.register(&w_cfg.model, engine);
                    }
                    Err(e) => {
                        warn!(model_id = %w_cfg.model, path = %tok_path, error = %e, "Failed to load worker tokenizer");
                    }
                }
            }
        }
    }

    let app_state = AppState {
        config: Arc::new(config.clone()),
        scheduler,
        tree,
        workers,
        tokenizer_registry,
        sessions: Arc::new(SessionLedger::new()),
        http_client: reqwest::Client::builder().build()?,
    };

    let app = Router::new()
        // Health & Diagnostics
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        // Cluster Admin & Diagnostics API
        .route("/api/v1/cluster/status", get(cluster_status_handler))
        // zene session linkage (docs/agent-inference-context.md)
        .route(
            "/v1/zene/sessions/{session_id}/publish",
            post(session_publish_handler),
        )
        .route("/v1/zene/sessions/{session_id}", delete(session_close_handler))
        // OpenAI-compatible endpoints
        .route("/v1/models", get(list_models_handler))
        .route("/v1/chat/completions", post(chat_completions_handler))
        .route("/v1/completions", post(chat_completions_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(app_state);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    info!("Cortex Gateway listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
