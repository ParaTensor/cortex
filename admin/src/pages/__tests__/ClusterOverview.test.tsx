import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import { ClusterOverview } from '../ClusterOverview'

describe('ClusterOverview Component', () => {
  beforeEach(() => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => ({
          total_workers: 2,
          ready_workers: 2,
          total_active_requests: 3,
          total_cached_blocks: 1540,
          total_sessions: 4,
          routing_stats: {
            exact_kv_events: 12,
            session_affinity: 3,
            load_aware: 0,
            fallback_p2c: 5,
            fallback_round_robin: 0,
            anchor_aligned_hits: 2,
            avg_exact_hit_pages: 18.5,
          },
          workers: [
            {
              id: 'sgl-worker-01',
              model: 'meta-llama/Llama-3.1-8B-Instruct',
              engine: 'sglang',
              role: 'standard',
              status: 'ready',
              http_endpoint: 'http://127.0.0.1:8001',
              zmq_endpoint: 'tcp://127.0.0.1:5557',
              active_requests: 2,
              last_seq: 450,
              last_heartbeat_ms_ago: 120,
            },
            {
              id: 'sgl-worker-02',
              model: 'meta-llama/Llama-3.1-8B-Instruct',
              engine: 'sglang',
              role: 'standard',
              status: 'ready',
              http_endpoint: 'http://127.0.0.1:8002',
              zmq_endpoint: 'tcp://127.0.0.1:5558',
              active_requests: 1,
              last_seq: 210,
              last_heartbeat_ms_ago: 90,
            },
          ],
        }),
      })
    )
  })

  it('renders cluster title and worker elements from API', async () => {
    render(<ClusterOverview />)
    expect(screen.getByText('集群概览')).toBeDefined()
    expect(await screen.findByText('sgl-worker-01')).toBeDefined()
    expect(await screen.findByText('sgl-worker-02')).toBeDefined()
    expect(await screen.findByText('真实 KV 命中')).toBeDefined()
    expect(screen.getAllByText('12').length).toBeGreaterThan(0)
    expect(screen.getAllByText('60%').length).toBeGreaterThan(0)
  })
})
