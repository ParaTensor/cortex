use std::sync::Arc;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use dashmap::DashMap;
use http_body_util::BodyExt;
use tower::ServiceExt;

use cortex::config::{CortexConfig, EngineType, SchedulerConfig, WorkerConfig, WorkerRole};
use cortex::hasher::TokenizerRegistry;
use cortex::ledger::{RadixHashTree, WorkerRuntimeState, WorkerSyncStatus};
use cortex::proxy::{chat_completions_handler, cluster_status_handler, list_models_handler, AppState};
use cortex::scheduler::LocalityScheduler;
use cortex::zmq::KvEventProcessor;

#[tokio::test]
async fn test_sglang_msgpack_event_processing_and_lru_removal() {
    let tree = Arc::new(RadixHashTree::new());
    let processor = KvEventProcessor::new(tree.clone());

    let worker_cfg = WorkerConfig {
        id: "sgl-gpu-test-01".to_string(),
        model: "llama-3".to_string(),
        engine: EngineType::Sglang,
        http_endpoint: "http://127.0.0.1:8001".to_string(),
        zmq_endpoint: Some("tcp://127.0.0.1:5557".to_string()),
        tokenizer_path: None,
        role: WorkerRole::Standard,
        page_size: 16,
        weight: 100,
    };
    let worker = WorkerRuntimeState::new(worker_cfg);

    // 1. SGLang msgspec msgpack wire representation for BlockStored:
    // [1.0, [["BlockStored", [1001, 1002, 1003], None, [1, 2, 3], 16, None, None]], 0]
    let seq1_bytes = 1u64.to_be_bytes();
    let stored_payload_hex = "93cb3ff00000000000009197ab426c6f636b53746f72656493cd03e9cd03eacd03ebc09301020310c0c000";
    let stored_payload_bytes = hex::decode(stored_payload_hex).unwrap();

    let events1 = KvEventProcessor::parse_sglang_multipart(&seq1_bytes, &stored_payload_bytes);
    assert_eq!(events1.len(), 1);
    assert_eq!(events1[0].seq, 1);
    processor.process_event(&worker, events1[0].clone());

    assert_eq!(*worker.status.read(), WorkerSyncStatus::Ready);
    assert_eq!(tree.total_cached_blocks(), 3);

    // 2. SGLang msgspec msgpack wire representation for BlockRemoved:
    // [2.0, [["BlockRemoved", [1003], None]], 0]
    let seq2_bytes = 2u64.to_be_bytes();
    let removed_payload_hex = "93cb40000000000000009193ac426c6f636b52656d6f76656491cd03ebc000";
    let removed_payload_bytes = hex::decode(removed_payload_hex).unwrap();

    let events2 = KvEventProcessor::parse_sglang_multipart(&seq2_bytes, &removed_payload_bytes);
    assert_eq!(events2.len(), 1);
    assert_eq!(events2[0].seq, 2);
    processor.process_event(&worker, events2[0].clone());

    assert_eq!(tree.total_cached_blocks(), 2);
}

#[tokio::test]
async fn test_axum_http_api_cluster_status_and_models() {
    let tree = Arc::new(RadixHashTree::new());
    let workers = Arc::new(DashMap::new());

    let worker_cfg = WorkerConfig {
        id: "worker-live-01".to_string(),
        model: "qwen-2.5".to_string(),
        engine: EngineType::Sglang,
        http_endpoint: "http://127.0.0.1:8001".to_string(),
        zmq_endpoint: None,
        tokenizer_path: None,
        role: WorkerRole::Standard,
        page_size: 16,
        weight: 100,
    };
    let worker_state = Arc::new(WorkerRuntimeState::new(worker_cfg));
    worker_state.set_status(WorkerSyncStatus::Ready);
    workers.insert("worker-live-01".to_string(), worker_state);

    let scheduler = Arc::new(LocalityScheduler::new(
        SchedulerConfig::default(),
        tree.clone(),
        workers.clone(),
    ));

    let app_state = AppState {
        config: Arc::new(CortexConfig::default()),
        scheduler,
        tree,
        workers,
        tokenizer_registry: Arc::new(TokenizerRegistry::new(100)),
        http_client: reqwest::Client::new(),
    };

    let app = Router::new()
        .route("/api/v1/cluster/status", get(cluster_status_handler))
        .route("/v1/models", get(list_models_handler))
        .route("/v1/chat/completions", post(chat_completions_handler))
        .with_state(app_state);

    // Test GET /api/v1/cluster/status
    let status_req = Request::builder()
        .uri("/api/v1/cluster/status")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(status_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body_bytes = res.into_body().collect().await.unwrap().to_bytes();
    let status_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(status_json["total_workers"], 1);
    assert_eq!(status_json["ready_workers"], 1);

    // Test GET /v1/models
    let models_req = Request::builder()
        .uri("/v1/models")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let models_res = app.oneshot(models_req).await.unwrap();
    assert_eq!(models_res.status(), StatusCode::OK);
}
