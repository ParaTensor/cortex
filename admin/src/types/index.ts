export type EngineType = 'sglang' | 'vllm' | 'dynamo'

export type WorkerRole = 'standard' | 'prefill' | 'decode'

export type WorkerStatus = 'init' | 'syncing' | 'ready' | 'stale'

export interface WorkerInfo {
  id: string
  model: string
  engine: EngineType
  role: WorkerRole
  status: WorkerStatus
  httpEndpoint: string
  zmqEndpoint?: string
  activeRequests: number
  cachedBlocks: number
  lastSeq: number
  lastHeartbeatMs: number
}

export interface ClusterStats {
  totalWorkers: number
  readyWorkers: number
  totalActiveRequests: number
  cacheHitRate: number
  exactKvRequests: number
  loadAwareRequests: number
  fallbackRequests: number
}
