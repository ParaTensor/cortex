use std::sync::Arc;
use dashmap::DashMap;

use cortex::config::{EngineType, SchedulerConfig, WorkerConfig, WorkerRole};
use cortex::hasher::compute_sglang_page_hashes;
use cortex::ledger::{RadixHashTree, WorkerRuntimeState, WorkerSyncStatus};
use cortex::scheduler::{LocalityScheduler, RoutingMode};
use cortex::zmq::{KvEventMessage, KvEventPayload, KvEventProcessor};

#[tokio::test]
async fn test_end_to_end_zmq_event_to_exact_kv_routing() {
    let tree = Arc::new(RadixHashTree::new());
    let workers = Arc::new(DashMap::new());

    let w1_cfg = WorkerConfig {
        id: "sgl-gpu-01".to_string(),
        model: "meta-llama/Llama-3.1-8B-Instruct".to_string(),
        engine: EngineType::Sglang,
        http_endpoint: "http://127.0.0.1:8001".to_string(),
        zmq_endpoint: Some("tcp://127.0.0.1:5557".to_string()),
        tokenizer_path: None,
        role: WorkerRole::Standard,
        page_size: 16,
        weight: 100,
    };
    let w2_cfg = WorkerConfig {
        id: "sgl-gpu-02".to_string(),
        model: "meta-llama/Llama-3.1-8B-Instruct".to_string(),
        engine: EngineType::Sglang,
        http_endpoint: "http://127.0.0.1:8002".to_string(),
        zmq_endpoint: Some("tcp://127.0.0.1:5558".to_string()),
        tokenizer_path: None,
        role: WorkerRole::Standard,
        page_size: 16,
        weight: 100,
    };

    let w1 = Arc::new(WorkerRuntimeState::new(w1_cfg));
    let w2 = Arc::new(WorkerRuntimeState::new(w2_cfg));

    workers.insert("sgl-gpu-01".to_string(), w1.clone());
    workers.insert("sgl-gpu-02".to_string(), w2.clone());

    let processor = Arc::new(KvEventProcessor::new(tree.clone()));
    let scheduler = LocalityScheduler::new(SchedulerConfig::default(), tree.clone(), workers.clone());

    // 1. Initial State: Workers are in INIT status, no KV in Radix tree
    let tokens: Vec<u32> = (100..164).collect();
    let page_hashes = compute_sglang_page_hashes(&tokens, 16);
    assert_eq!(page_hashes.len(), 4);

    // Initial query should fallback to P2C / load-aware because workers are not READY
    let init_decision = scheduler
        .select_worker("meta-llama/Llama-3.1-8B-Instruct", &page_hashes, None)
        .unwrap();
    assert_eq!(init_decision.mode, RoutingMode::FallbackP2c);
    assert_eq!(init_decision.matched_pages, 0);

    // 2. Simulate Worker 1 broadcasting BlockStored events over ZMQ
    processor.process_event(
        &w1,
        KvEventMessage {
            seq: 1,
            payload: KvEventPayload::BlockStored {
                page_hashes: page_hashes.clone(),
            },
        },
    );
    assert_eq!(*w1.status.read(), WorkerSyncStatus::Ready);

    // 3. Query again: should match 4 pages on sgl-gpu-01 with exact_kv_events
    let matched_decision = scheduler
        .select_worker("meta-llama/Llama-3.1-8B-Instruct", &page_hashes, None)
        .unwrap();
    assert_eq!(matched_decision.worker_id, "sgl-gpu-01");
    assert_eq!(matched_decision.mode, RoutingMode::ExactKvEvents);
    assert_eq!(matched_decision.matched_pages, 4);

    // 4. Simulate Worker 1 Cache Eviction (AllBlocksCleared)
    processor.process_event(
        &w1,
        KvEventMessage {
            seq: 2,
            payload: KvEventPayload::AllBlocksCleared,
        },
    );

    // 5. Query after eviction: should miss and cleanly fallback
    let after_evict_decision = scheduler
        .select_worker("meta-llama/Llama-3.1-8B-Instruct", &page_hashes, None)
        .unwrap();
    assert_eq!(after_evict_decision.matched_pages, 0);
    assert_eq!(after_evict_decision.mode, RoutingMode::FallbackP2c);
}
