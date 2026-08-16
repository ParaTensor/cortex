use std::sync::Arc;
use std::time::Instant;
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
use crate::hasher::{compute_sglang_page_hashes, ChatMessage, TokenizerRegistry};
use crate::ledger::{RadixHashTree, WorkerRuntimeState};
use crate::scheduler::LocalityScheduler;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<CortexConfig>,
    pub scheduler: Arc<LocalityScheduler>,
    pub tree: Arc<RadixHashTree>,
    pub workers: Arc<dashmap::DashMap<String, Arc<WorkerRuntimeState>>>,
    pub tokenizer_registry: Arc<TokenizerRegistry>,
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

    let page_size = 16;

    // 1. Tokenize messages or prompt with Fast Tokenizer & LRU Cache
    let mut page_hashes = Vec::new();

    if let Some(messages_val) = payload.get("messages").and_then(|m| m.as_array()) {
        let mut chat_messages = Vec::with_capacity(messages_val.len());
        for msg in messages_val {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
            chat_messages.push(ChatMessage {
                role: role.to_string(),
                content: content.to_string(),
            });
        }

        if let Some(token_ids) = state.tokenizer_registry.tokenize_chat(model, &chat_messages) {
            page_hashes = compute_sglang_page_hashes(&token_ids, page_size);
        }
    } else if let Some(prompt) = payload.get("prompt").and_then(|p| p.as_str()) {
        if let Some(token_ids) = state.tokenizer_registry.tokenize_text(model, prompt) {
            page_hashes = compute_sglang_page_hashes(&token_ids, page_size);
        }
    }

    // 2. Schedule request using 4-tier fallback
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
        HeaderValue::from_str(&(decision.matched_pages * page_size).to_string())
            .unwrap_or(HeaderValue::from_static("0")),
    );

    // Stream the body with automatic decrement on finish
    let worker_guard = worker.clone();
    let stream = upstream_res.bytes_stream().map(move |item| {
        item.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    });

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

pub async fn cluster_status_handler(State(state): State<AppState>) -> impl IntoResponse {
    let mut total_active = 0;
    let mut ready_count = 0;
    let mut worker_list = Vec::new();
    let now = Instant::now();

    for entry in state.workers.iter() {
        let w = entry.value();
        let status = *w.status.read();
        let active = w.get_active_requests();
        total_active += active;

        let status_str = match status {
            crate::ledger::WorkerSyncStatus::Init => "init",
            crate::ledger::WorkerSyncStatus::Syncing => "syncing",
            crate::ledger::WorkerSyncStatus::Ready => {
                ready_count += 1;
                "ready"
            }
            crate::ledger::WorkerSyncStatus::Stale => "stale",
        };

        let last_hb = *w.last_heartbeat.read();
        let hb_ago_ms = now.saturating_duration_since(last_hb).as_millis() as u64;

        worker_list.push(serde_json::json!({
            "id": w.config.id,
            "model": w.config.model,
            "engine": serde_json::to_value(w.config.engine).unwrap_or(Value::String("sglang".to_string())),
            "role": serde_json::to_value(w.config.role).unwrap_or(Value::String("standard".to_string())),
            "status": status_str,
            "http_endpoint": w.config.http_endpoint,
            "zmq_endpoint": w.config.zmq_endpoint,
            "active_requests": active,
            "last_seq": *w.last_seq.read(),
            "last_heartbeat_ms_ago": hb_ago_ms,
        }));
    }

    let total_blocks = state.tree.total_cached_blocks();

    Json(serde_json::json!({
        "total_workers": state.workers.len(),
        "ready_workers": ready_count,
        "total_active_requests": total_active,
        "total_cached_blocks": total_blocks,
        "workers": worker_list,
    }))
}
