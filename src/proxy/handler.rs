use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Response, StatusCode},
    response::IntoResponse,
};
use futures_util::StreamExt;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info, warn};

use crate::config::CortexConfig;
use crate::hasher::{ChatMessage, TokenizerRegistry};
use crate::ledger::{RadixHashTree, WorkerRuntimeState, WorkerSyncStatus};
use crate::metrics::RoutingStats;
use crate::scheduler::{LocalityScheduler, RoutingMode, SchedulingDecision};
use crate::session_ledger::{SessionLedger, SessionPublishRequest};

/// zene outbound session headers (docs/agent-inference-context.md).
const HEADER_ZENE_SESSION: &str = "x-zene-session-id";
const HEADER_ZENE_EPOCH: &str = "x-zene-context-epoch";

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<CortexConfig>,
    pub scheduler: Arc<LocalityScheduler>,
    pub tree: Arc<RadixHashTree>,
    pub workers: Arc<dashmap::DashMap<String, Arc<WorkerRuntimeState>>>,
    pub tokenizer_registry: Arc<TokenizerRegistry>,
    pub sessions: Arc<SessionLedger>,
    pub routing_stats: Arc<RoutingStats>,
    pub http_client: reqwest::Client,
}

/// Injects gateway routing telemetry into a JSON object in-place:
/// top-level `cortex` plus `usage.gateway_*` mirrors (zene issue #128).
fn inject_cortex_telemetry(
    payload: &mut Value,
    worker_id: &str,
    mode: &str,
    hit_tokens: usize,
    anchor_aligned: bool,
) {
    let Some(obj) = payload.as_object_mut() else {
        return;
    };
    obj.insert(
        "cortex".to_string(),
        serde_json::json!({
            "assigned_worker": worker_id,
            "match_mode": mode,
            "cache_hit_tokens": hit_tokens,
            "anchor_aligned": anchor_aligned,
        }),
    );
    if let Some(usage) = obj.get_mut("usage").and_then(|u| u.as_object_mut()) {
        usage.insert(
            "gateway_cache_hit_tokens".to_string(),
            serde_json::json!(hit_tokens),
        );
        // Alias consumed by unigateway-sdk's cache-hit normalizer.
        usage.insert(
            "cache_hit_tokens".to_string(),
            serde_json::json!(hit_tokens),
        );
        usage.insert(
            "gateway_anchor_aligned".to_string(),
            serde_json::json!(anchor_aligned),
        );
    }
}

/// Rewrites a single SSE `data: {...}` line when it carries a `usage` object
/// so streaming clients (zene's default path) receive the same telemetry as
/// non-streaming JSON. Lines that are not JSON usage frames pass through.
fn rewrite_sse_data_line(
    line: &str,
    worker_id: &str,
    mode: &str,
    hit_tokens: usize,
    anchor_aligned: bool,
) -> String {
    let Some(payload) = line.strip_prefix("data: ") else {
        return line.to_string();
    };
    if payload == "[DONE]" {
        return line.to_string();
    }
    let Ok(mut value) = serde_json::from_str::<Value>(payload) else {
        return line.to_string();
    };
    if !value.get("usage").map(|u| u.is_object()).unwrap_or(false) {
        return line.to_string();
    }
    inject_cortex_telemetry(&mut value, worker_id, mode, hit_tokens, anchor_aligned);
    match serde_json::to_string(&value) {
        Ok(json) => format!("data: {json}"),
        Err(_) => line.to_string(),
    }
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

    // 1. Tokenize messages or prompt with Fast Tokenizer & Zero-Allocation LRU Cache
    let (page_hashes, page_is_anchor): (Arc<Vec<i64>>, Arc<Vec<bool>>) =
        if let Some(messages_val) = payload.get("messages").and_then(|m| m.as_array()) {
            let mut chat_messages = Vec::with_capacity(messages_val.len());
            for msg in messages_val {
                let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
                let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let has_tool_calls = msg
                    .get("tool_calls")
                    .and_then(|t| t.as_array())
                    .is_some_and(|a| !a.is_empty());
                chat_messages.push(ChatMessage {
                    role: role.to_string(),
                    content: content.to_string(),
                    has_tool_calls,
                });
            }

            // Tool schema participates in engine-side template rendering; hashes
            // must be computed over the same stream or exact matching breaks.
            let tools_val = payload.get("tools").filter(|t| t.is_array());
            state
                .tokenizer_registry
                .tokenize_and_hash_chat_with_tools(model, &chat_messages, tools_val, page_size)
                .map(|out| (out.page_hashes.clone(), out.page_is_anchor.clone()))
                .unwrap_or_else(|| (Arc::new(Vec::new()), Arc::new(Vec::new())))
        } else if let Some(prompt) = payload.get("prompt").and_then(|p| p.as_str()) {
            state
                .tokenizer_registry
                .tokenize_and_hash_text(model, prompt, page_size)
                .map(|out| (out.page_hashes.clone(), out.page_is_anchor.clone()))
                .unwrap_or_else(|| (Arc::new(Vec::new()), Arc::new(Vec::new())))
        } else {
            (Arc::new(Vec::new()), Arc::new(Vec::new()))
        };

    // 2. Schedule request using 4-tier fallback
    let mut decision =
        match state
            .scheduler
            .select_worker(model, &page_hashes, &page_is_anchor, None)
        {
            Some(d) => d,
            None => {
                warn!(model = %model, "No available worker found for request");
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
        };

    // 2b. Session affinity override (zene linkage): when the agent-declared
    // epoch matches the published baseline and Tier-1 exact matching found
    // nothing, prefer the worker that served the previous turn — its engine
    // radix cache almost certainly still holds the canonical prefix even when
    // our ZMQ ledger is cold.
    let zene_session = headers
        .get(HEADER_ZENE_SESSION)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let zene_epoch = headers
        .get(HEADER_ZENE_EPOCH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    if let (Some(session_id), Some(epoch)) = (&zene_session, zene_epoch) {
        if matches!(
            decision.mode,
            RoutingMode::FallbackP2c | RoutingMode::FallbackRoundRobin | RoutingMode::LoadAware
        ) {
            if let Some(sticky_id) = state.sessions.sticky_worker(session_id, epoch) {
                let sticky_ok = state.workers.get(&sticky_id).is_some_and(|w| {
                    w.config.model == model
                        && w.get_active_requests()
                            < state.config.scheduler.max_active_requests_per_worker
                });
                if sticky_ok {
                    if let Some(w) = state.workers.get(&sticky_id) {
                        decision = SchedulingDecision {
                            worker_id: sticky_id.clone(),
                            http_endpoint: w.config.http_endpoint.clone(),
                            matched_pages: 0,
                            mode: RoutingMode::SessionAffinity,
                            anchor_aligned: false,
                        };
                    }
                }
            }
        }
        state
            .sessions
            .record_assignment(session_id, epoch, &decision.worker_id);
    }

    let mode_str = decision.mode.as_str();
    state.routing_stats.record_mode(mode_str);
    if decision.mode == RoutingMode::ExactKvEvents {
        state
            .routing_stats
            .record_exact_hit(decision.matched_pages, decision.anchor_aligned);
    }

    let worker = match state.workers.get(&decision.worker_id) {
        Some(w) => w.clone(),
        None => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    // Increment active request count
    worker.inc_active_requests();

    let target_url = format!(
        "{}/v1/chat/completions",
        decision.http_endpoint.trim_end_matches('/')
    );

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

    // Write-through: the engine just prefills this prefix on the assigned
    // worker. Insert the hashes we already computed so the next identical
    // request exact-matches immediately — without waiting for ZMQ flush
    // (idle-only on some SGLang loops) and without depending on the engine
    // re-emitting already-cached blocks after a gateway restart.
    // ZMQ BlockRemoved remains the eviction authority.
    if status.is_success() && !page_hashes.is_empty() {
        state.tree.insert_chain(&decision.worker_id, &page_hashes);
        if let Some(w) = state.workers.get(&decision.worker_id) {
            let st = *w.status.read();
            if st == WorkerSyncStatus::Syncing || st == WorkerSyncStatus::Init {
                w.set_status(WorkerSyncStatus::Ready);
            }
        }
    }

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
    response_headers.insert(
        "x-cortex-anchor-aligned",
        HeaderValue::from_static(if decision.anchor_aligned {
            "true"
        } else {
            "false"
        }),
    );

    // Inject routing metadata into the response body for non-streaming JSON
    // responses (unigateway-sdk's proxy_chat does not surface headers, so
    // agents consume gateway telemetry from the body; zene issue #128).
    // Streaming (SSE) responses keep header-only metadata: usage appears in
    // the final chunk and rewriting SSE frames is not worth the complexity.
    let is_json = upstream_res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"));

    let worker_guard = worker.clone();
    let body = if is_json {
        // Body length changes after injection; drop the upstream length so
        // hyper recomputes it.
        response_headers.remove("content-length");
        let bytes = match upstream_res.bytes().await {
            Ok(b) => b,
            Err(e) => {
                error!(worker_id = %decision.worker_id, error = %e, "Failed to read upstream body");
                worker.dec_active_requests();
                return Err(StatusCode::BAD_GATEWAY);
            }
        };
        // Nothing async remains: release the slot eagerly.
        drop(worker_guard);
        worker.dec_active_requests();

        let mut payload: Value = match serde_json::from_slice(&bytes) {
            Ok(v @ Value::Object(_)) => v,
            _ => Value::Null,
        };
        if payload.is_object() {
            let hit_tokens = decision.matched_pages * page_size;
            inject_cortex_telemetry(
                &mut payload,
                &decision.worker_id,
                mode_str,
                hit_tokens,
                decision.anchor_aligned,
            );
            Body::from(serde_json::to_vec(&payload).unwrap_or_else(|_| bytes.to_vec()))
        } else {
            Body::from(bytes)
        }
    } else {
        // Streaming (SSE): rewrite frames that carry a `usage` object so
        // agents on the default streaming path still receive gateway
        // telemetry (zene sets stream_options.include_usage).
        response_headers.remove("content-length");
        let stream = upstream_res
            .bytes_stream()
            .map(move |item| item.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)));
        let worker_id = decision.worker_id.clone();
        let mode = mode_str.to_string();
        let hit_tokens = decision.matched_pages * page_size;
        let aligned = decision.anchor_aligned;

        let stream_with_cleanup = async_stream::stream! {
            let _guard = scopeguard::guard(worker_guard, |w| {
                w.dec_active_requests();
            });

            tokio::pin!(stream);
            let mut leftover = Vec::<u8>::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        leftover.extend_from_slice(&bytes);
                        while let Some(pos) = leftover.iter().position(|&b| b == b'\n') {
                            let mut line_bytes: Vec<u8> = leftover.drain(..=pos).collect();
                            if line_bytes.last() == Some(&b'\n') {
                                line_bytes.pop();
                            }
                            if line_bytes.last() == Some(&b'\r') {
                                line_bytes.pop();
                            }
                            let line = String::from_utf8_lossy(&line_bytes);
                            let rewritten = rewrite_sse_data_line(
                                &line, &worker_id, &mode, hit_tokens, aligned,
                            );
                            yield Ok(bytes::Bytes::from(format!("{rewritten}\n")));
                        }
                    }
                    Err(e) => {
                        yield Err(e);
                        break;
                    }
                }
            }
            if !leftover.is_empty() {
                let line = String::from_utf8_lossy(&leftover);
                let rewritten = rewrite_sse_data_line(
                    &line, &worker_id, &mode, hit_tokens, aligned,
                );
                yield Ok(bytes::Bytes::from(rewritten));
            }
        };

        Body::from_stream(stream_with_cleanup)
    };
    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = response_headers;

    Ok(response)
}

/// zene linkage: agent publishes its canonical prefix baseline after a
/// compaction / system resize (epoch++). See docs/agent-inference-context.md.
/// This is the cold-start snapshot channel: a freshly (re)started gateway
/// learns the session's anchor boundaries and fingerprint without replaying
/// traffic, and re-arms sticky affinity on the next turn.
pub async fn session_publish_handler(
    State(state): State<AppState>,
    axum::extract::Path(session_id): axum::extract::Path<String>,
    Json(req): Json<SessionPublishRequest>,
) -> impl IntoResponse {
    let accepted = state.sessions.publish(&session_id, &req);
    if accepted {
        info!(
            session_id = %session_id,
            epoch = req.epoch,
            message_count = req.message_count,
            anchors = req.anchor_boundaries.as_ref().map(|a| a.len()).unwrap_or(0),
            "zene session baseline published"
        );
        StatusCode::OK
    } else {
        // Stale epoch delivery; the agent should treat this as idempotent success.
        warn!(session_id = %session_id, epoch = req.epoch, "stale session publish rejected");
        StatusCode::CONFLICT
    }
}

/// zene linkage: run teardown. Drops routing metadata for the session.
pub async fn session_close_handler(
    State(state): State<AppState>,
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if state.sessions.close(&session_id) {
        info!(session_id = %session_id, "zene session closed");
    }
    StatusCode::NO_CONTENT
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
        "total_sessions": state.sessions.total_sessions(),
        "routing_stats": state.routing_stats.to_json(),
        "workers": worker_list,
    }))
}
