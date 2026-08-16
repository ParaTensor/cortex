use std::net::SocketAddr;
use std::sync::Arc;
use axum::{
    routing::{get, post},
    Router,
};
use clap::Parser;
use dashmap::DashMap;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

use cortex::config::CortexConfig;
use cortex::ledger::{RadixHashTree, WorkerRuntimeState};
use cortex::metrics::{health_live, health_ready};
use cortex::proxy::{chat_completions_handler, list_models_handler, AppState};
use cortex::scheduler::LocalityScheduler;

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

    let scheduler = Arc::new(LocalityScheduler::new(
        config.scheduler.clone(),
        tree.clone(),
        workers.clone(),
    ));

    let app_state = AppState {
        config: Arc::new(config.clone()),
        scheduler,
        tree,
        workers,
        http_client: reqwest::Client::builder().build()?,
    };

    let app = Router::new()
        // Health & Diagnostics
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
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
