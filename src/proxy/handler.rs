use std::sync::Arc;
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Response, StatusCode},
    response::IntoResponse,
    Json,
};
use futures_util::StreamExt;
use serde_json::Value;
use tracing::{error, warn};

use crate::config::CortexConfig;
use crate::hasher::compute_sglang_page_hashes;
use crate::ledger::{RadixHashTree, WorkerRuntimeState};
use crate::scheduler::LocalityScheduler;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<CortexConfig>,
    pub scheduler: Arc<LocalityScheduler>,
    pub tree: Arc<RadixHashTree>,
    pub workers: Arc<dashmap::DashMap<String, Arc<WorkerRuntimeState>>>,
    pub http_client: reqwest::Client,
}

pub async fn chat_completions_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response<Body>, StatusCode> {
    let model = payload
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("default");

    // In MVP phase: extract token IDs if provided in header or compute mock hashes from prompt
    // In full tokenizer phase: tokenization is invoked on prompt
    let mut page_hashes = Vec::new();
    if let Some(prompt) = payload.get("prompt").and_then(|p| p.as_str()) {
        // Simple mock tokenization for initial MVP test (4 chars per token)
        let token_ids: Vec<u32> = prompt.as_bytes().iter().map(|&b| b as u32).collect();
        page_hashes = compute_sglang_page_hashes(&token_ids, 16);
    }

    let decision = match state.scheduler.select_worker(model, &page_hashes, None) {
        Some(d) => d,
        None => {
            warn!(model = %model, "No available worker found for request");
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
    };

    let worker = match state.workers.get(&decision.worker_id) {
        Some(w) => w.clone(),
        None => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    // Increment active request count
    worker.inc_active_requests();

    let target_url = format!("{}/v1/chat/completions", decision.http_endpoint.trim_end_matches('/'));

    let mut req_builder = state.http_client.post(&target_url).json(&payload);
    for (k, v) in headers.iter() {
        if k != "host" && k != "content-length" {
            req_builder = req_builder.header(k.as_str(), v.as_bytes());
        }
    }

    let upstream_res = match req_builder.send().await {
        Ok(res) => res,
        Err(err) => {
            error!(worker_id = %decision.worker_id, error = %err, "Upstream connection failed");
            worker.dec_active_requests();
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

    let status = StatusCode::from_u16(upstream_res.status().as_u16()).unwrap_or(StatusCode::OK);
    let mut response_headers = HeaderMap::new();

    for (k, v) in upstream_res.headers() {
        if let Ok(name) = axum::http::header::HeaderName::from_bytes(k.as_str().as_bytes()) {
            if let Ok(val) = HeaderValue::from_bytes(v.as_bytes()) {
                response_headers.insert(name, val);
            }
        }
    }

    // Inject Cortex diagnostic headers for XRouter & observability
    response_headers.insert(
        "x-cortex-assigned-worker",
        HeaderValue::from_str(&decision.worker_id).unwrap_or(HeaderValue::from_static("unknown")),
    );
    response_headers.insert(
        "x-cortex-match-mode",
        HeaderValue::from_static(decision.mode.as_str()),
    );
    response_headers.insert(
        "x-cortex-cache-hit-tokens",
        HeaderValue::from_str(&(decision.matched_pages * 16).to_string())
            .unwrap_or(HeaderValue::from_static("0")),
    );

    // Stream the body with automatic decrement on finish
    let worker_guard = worker.clone();
    let stream = upstream_res.bytes_stream().map(move |item| {
        item.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    });

    // Wrap stream to ensure decrementing active requests when stream ends or drops
    let stream_with_cleanup = async_stream::stream! {
        let _guard = scopeguard::guard(worker_guard, |w| {
            w.dec_active_requests();
        });

        tokio::pin!(stream);
        while let Some(chunk) = stream.next().await {
            yield chunk;
        }
    };

    let body = Body::from_stream(stream_with_cleanup);
    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = response_headers;

    Ok(response)
}

pub async fn list_models_handler(State(state): State<AppState>) -> impl IntoResponse {
    let mut models = Vec::new();
    for entry in state.workers.iter() {
        let w = entry.value();
        if !models.contains(&w.config.model) {
            models.push(w.config.model.clone());
        }
    }

    let data: Vec<Value> = models
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "id": m,
                "object": "model",
                "owned_by": "cortex-cluster",
            })
        })
        .collect();

    Json(serde_json::json!({
        "object": "list",
        "data": data
    }))
}
