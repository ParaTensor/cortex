mod routing_stats;

pub use routing_stats::RoutingStats;

use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::json;

pub async fn health_live() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "live" })))
}

pub async fn health_ready() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ready" })))
}
