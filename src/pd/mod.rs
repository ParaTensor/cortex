use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SglangBootstrapInfo {
    pub host: String,
    pub port: u16,
    pub room: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VllmKvTransferParams {
    pub session_id: String,
    pub engine_rank: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PdTransferMetadata {
    Sglang(SglangBootstrapInfo),
    Vllm(VllmKvTransferParams),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdSessionState {
    Init,
    PrefillScheduled,
    PrefillCompleted,
    DecodeScheduled,
    Streaming,
    Completed,
    Failed,
}

pub struct PdSession {
    pub session_id: String,
    pub model: String,
    pub state: PdSessionState,
    pub prefill_worker_id: Option<String>,
    pub decode_worker_id: Option<String>,
    pub metadata: Option<PdTransferMetadata>,
}

impl PdSession {
    pub fn new(session_id: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            model: model.into(),
            state: PdSessionState::Init,
            prefill_worker_id: None,
            decode_worker_id: None,
            metadata: None,
        }
    }
}
