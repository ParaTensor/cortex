use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    #[serde(default)]
    pub workers: Vec<WorkerConfig>,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model_id: String,
    pub tokenizer_path: Option<String>,
    pub chat_template: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineType {
    Sglang,
    Vllm,
    Dynamo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    Standard,
    Prefill,
    Decode,
}

impl Default for WorkerRole {
    fn default() -> Self {
        Self::Standard
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub id: String,
    pub model: String,
    pub engine: EngineType,
    pub http_endpoint: String,
    pub zmq_endpoint: Option<String>,
    pub tokenizer_path: Option<String>,
    #[serde(default)]
    pub role: WorkerRole,
    #[serde(default = "default_page_size")]
    pub page_size: usize,
    #[serde(default = "default_weight")]
    pub weight: u32,
}

fn default_page_size() -> usize {
    16
}

fn default_weight() -> u32 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    #[serde(default = "default_kv_weight")]
    pub kv_weight: f64,
    #[serde(default = "default_load_weight")]
    pub load_weight: f64,
    #[serde(default = "default_high_watermark")]
    pub max_active_requests_per_worker: usize,
    /// Threshold difference of active requests to trigger P2C or load-aware fallback
    #[serde(default = "default_enable_p2c")]
    pub enable_p2c: bool,
}

fn default_kv_weight() -> f64 {
    1.0
}

fn default_load_weight() -> f64 {
    0.5
}

fn default_high_watermark() -> usize {
    64
}

fn default_enable_p2c() -> bool {
    true
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            kv_weight: default_kv_weight(),
            load_weight: default_load_weight(),
            max_active_requests_per_worker: default_high_watermark(),
            enable_p2c: default_enable_p2c(),
        }
    }
}

impl Default for CortexConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            models: vec![],
            workers: vec![],
            scheduler: SchedulerConfig::default(),
        }
    }
}
