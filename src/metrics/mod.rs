mod routing_stats;

pub use routing_stats::RoutingStats;

use axum::{http::StatusCode, response::IntoResponse, Json};
use serde_json::json;

pub async fn health_live() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "live" })))
}

pub async fn health_ready() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ready" })))
}
