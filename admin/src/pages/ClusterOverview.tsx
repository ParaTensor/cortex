import React, { useState, useEffect } from 'react'
import { Server, Activity, Zap, RefreshCw, Search, ShieldCheck } from 'lucide-react'
import { useI18n } from '@/lib/i18n'
import type { RoutingStats, WorkerInfo } from '@/types'
import { cn } from '@/lib/utils'

interface ClusterApiResponse {
  total_workers: number
  ready_workers: number
  total_active_requests: number
  total_cached_blocks: number
  total_sessions?: number
  routing_stats?: {
    exact_kv_events: number
    session_affinity: number
    load_aware: number
    fallback_p2c: number
    fallback_round_robin: number
    anchor_aligned_hits: number
    avg_exact_hit_pages: number
  }
  workers: Array<{
    id: string
    model: string
    engine: 'sglang' | 'vllm' | 'dynamo'
    role: 'standard' | 'prefill' | 'decode'
    status: 'init' | 'syncing' | 'ready' | 'stale'
    http_endpoint: string
    zmq_endpoint?: string
    active_requests: number
    last_seq: number
    last_heartbeat_ms_ago: number
  }>
}

export const ClusterOverview: React.FC = () => {
  const { t } = useI18n()
  const [searchTerm, setSearchTerm] = useState('')
  const [sortBy, setSortBy] = useState<'active' | 'blocks'>('active')
  const [workers, setWorkers] = useState<WorkerInfo[]>([])
  const [totalCachedBlocks, setTotalCachedBlocks] = useState(0)
  const [totalSessions, setTotalSessions] = useState(0)
  const [routing, setRouting] = useState<RoutingStats>({
    exactKvEvents: 0,
    sessionAffinity: 0,
    loadAware: 0,
    fallbackP2c: 0,
    fallbackRoundRobin: 0,
    anchorAlignedHits: 0,
    avgExactHitPages: 0,
  })
  const [loading, setLoading] = useState(true)

  const fetchClusterStatus = async () => {
    try {
      const res = await fetch('/api/v1/cluster/status')
      if (res.ok) {
        const data: ClusterApiResponse = await res.json()
        setTotalCachedBlocks(data.total_cached_blocks || 0)
        setTotalSessions(data.total_sessions || 0)
        const rs = data.routing_stats
        if (rs) {
          setRouting({
            exactKvEvents: rs.exact_kv_events || 0,
            sessionAffinity: rs.session_affinity || 0,
            loadAware: rs.load_aware || 0,
            fallbackP2c: rs.fallback_p2c || 0,
            fallbackRoundRobin: rs.fallback_round_robin || 0,
            anchorAlignedHits: rs.anchor_aligned_hits || 0,
            avgExactHitPages: rs.avg_exact_hit_pages || 0,
          })
        }
        setWorkers(
          data.workers.map((w) => ({
            id: w.id,
            model: w.model,
            engine: w.engine,
            role: w.role,
            status: w.status,
            httpEndpoint: w.http_endpoint,
            zmqEndpoint: w.zmq_endpoint,
            activeRequests: w.active_requests,
            cachedBlocks: 0,
            lastSeq: w.last_seq,
            lastHeartbeatMs: w.last_heartbeat_ms_ago,
          }))
        )
      }
    } catch {
      // Fallback for isolated offline testing
    } finally {
      setLoading(false)
    }
  };

  useEffect(() => {
    fetchClusterStatus()
    const timer = setInterval(fetchClusterStatus, 3000)
    return () => clearInterval(timer)
  }, [])

  const filteredWorkers = workers
    .filter(
      (w) =>
        w.id.toLowerCase().includes(searchTerm.toLowerCase()) ||
        w.model.toLowerCase().includes(searchTerm.toLowerCase()) ||
        w.httpEndpoint.toLowerCase().includes(searchTerm.toLowerCase())
    )
    .sort((a, b) => (sortBy === 'active' ? b.activeRequests - a.activeRequests : b.cachedBlocks - a.cachedBlocks))

  const totalActive = workers.reduce((acc, w) => acc + w.activeRequests, 0)
  const readyCount = workers.filter((w) => w.status === 'ready').length
  const routedTotal =
    routing.exactKvEvents +
    routing.sessionAffinity +
    routing.loadAware +
    routing.fallbackP2c +
    routing.fallbackRoundRobin
  const exactHitPct = routedTotal > 0 ? Math.round((routing.exactKvEvents / routedTotal) * 100) : 0
  const routingRows = [
    { key: 'exact', label: t.metrics.exactKv, value: routing.exactKvEvents },
    { key: 'affinity', label: t.metrics.sessionAffinity, value: routing.sessionAffinity },
    { key: 'load', label: t.metrics.loadAware, value: routing.loadAware },
    { key: 'p2c', label: t.metrics.fallbackP2c, value: routing.fallbackP2c },
    { key: 'rr', label: t.metrics.fallbackRr, value: routing.fallbackRoundRobin },
  ]

  const getStatusBadge = (status: WorkerInfo['status']) => {
    switch (status) {
      case 'ready':
        return <span className="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-success/15 text-success">{t.status.ready}</span>
      case 'syncing':
        return <span className="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-warning/15 text-warning">{t.status.syncing}</span>
      case 'stale':
        return <span className="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-destructive/15 text-destructive">{t.status.stale}</span>
      default:
        return <span className="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-muted text-muted-foreground">{t.status.init}</span>
    }
  }

  return (
    <div className="space-y-6">
      {/* Top Bar Header */}
      <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-4">
        <div>
          <h1 className="text-xl font-bold tracking-tight text-foreground">{t.common.cluster}</h1>
          <p className="text-xs text-muted-foreground mt-0.5">{t.cluster.subtitle}</p>
        </div>
        <button
          onClick={fetchClusterStatus}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium bg-primary text-primary-foreground rounded-md hover:bg-primary/90 transition-colors shadow-xs cursor-pointer"
        >
          <RefreshCw className={cn("w-3.5 h-3.5", loading && "animate-spin")} />
          {t.common.refresh}
        </button>
      </div>

      {/* Metrics Grid */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
        <div className="bg-card border border-border rounded-lg p-4 shadow-2xs">
          <div className="flex items-center justify-between text-muted-foreground">
            <span className="text-xs font-medium">{t.metrics.totalWorkers}</span>
            <Server className="w-4 h-4" />
          </div>
          <div className="mt-2 flex items-baseline gap-2">
            <span className="text-2xl font-bold tracking-tight">{workers.length}</span>
            <span className="text-xs text-muted-foreground">({readyCount} {t.status.ready})</span>
          </div>
        </div>

        <div className="bg-card border border-border rounded-lg p-4 shadow-2xs">
          <div className="flex items-center justify-between text-muted-foreground">
            <span className="text-xs font-medium">{t.metrics.clusterLoad}</span>
            <Activity className="w-4 h-4" />
          </div>
          <div className="mt-2 flex items-baseline gap-2">
            <span className="text-2xl font-bold tracking-tight text-primary">{totalActive}</span>
            <span className="text-xs text-muted-foreground">{t.metrics.concurrentSuffix}</span>
          </div>
        </div>

        <div className="bg-card border border-border rounded-lg p-4 shadow-2xs">
          <div className="flex items-center justify-between text-muted-foreground">
            <span className="text-xs font-medium">{t.common.cachedBlocks}</span>
            <Zap className="w-4 h-4 text-warning" />
          </div>
          <div className="mt-2 flex items-baseline gap-2">
            <span className="text-2xl font-bold tracking-tight text-foreground">{totalCachedBlocks}</span>
            <span className="text-xs text-muted-foreground">{t.metrics.radixNodes}</span>
          </div>
        </div>

        <div className="bg-card border border-border rounded-lg p-4 shadow-2xs">
          <div className="flex items-center justify-between text-muted-foreground">
            <span className="text-xs font-medium">{t.metrics.exactHitRate}</span>
            <ShieldCheck className="w-4 h-4 text-primary" />
          </div>
          <div className="mt-2 flex items-baseline gap-2">
            <span className="text-2xl font-bold tracking-tight text-primary">{exactHitPct}%</span>
            <span className="text-xs text-muted-foreground">
              {routing.exactKvEvents}/{routedTotal} {t.metrics.routedRequests}
            </span>
          </div>
        </div>
      </div>

      <div className="bg-card border border-border rounded-lg p-4 shadow-2xs">
        <div className="flex items-center justify-between mb-3">
          <span className="text-xs font-medium text-muted-foreground">{t.metrics.routingBreakdown}</span>
          <span className="text-xs text-muted-foreground">
            {t.metrics.totalSessions}: <span className="font-semibold text-foreground">{totalSessions}</span>
          </span>
        </div>
        <div className="space-y-2">
          {routingRows.map((row) => (
            <RoutingBar key={row.key} label={row.label} value={row.value} total={routedTotal} />
          ))}
        </div>
        <div className="mt-3 pt-3 border-t border-border grid grid-cols-2 gap-3 text-xs">
          <div className="flex justify-between gap-2">
            <span className="text-muted-foreground">{t.metrics.anchorAligned}</span>
            <span className="font-semibold tabular-nums">{routing.anchorAlignedHits}</span>
          </div>
          <div className="flex justify-between gap-2">
            <span className="text-muted-foreground">{t.metrics.avgExactPages}</span>
            <span className="font-semibold tabular-nums">{routing.avgExactHitPages.toFixed(1)}</span>
          </div>
        </div>
      </div>

      {/* Workers Management Section */}
      <div className="bg-card border border-border rounded-lg shadow-2xs overflow-hidden">
        {/* Table Filters Header */}
        <div className="p-4 border-b border-border flex flex-col sm:flex-row sm:items-center justify-between gap-3 bg-card">
          <div className="relative flex-1 max-w-md">
            <Search className="w-4 h-4 absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground" />
            <input
              type="text"
              placeholder={t.common.searchPlaceholder}
              value={searchTerm}
              onChange={(e) => setSearchTerm(e.target.value)}
              className="w-full pl-9 pr-3 py-1.5 text-xs bg-background border border-input rounded-md focus:outline-hidden focus:ring-1 focus:ring-ring"
            />
          </div>

          <div className="flex items-center gap-2">
            <span className="text-xs text-muted-foreground">{t.cluster.sortLabel}</span>
            <select
              value={sortBy}
              onChange={(e) => setSortBy(e.target.value as 'active' | 'blocks')}
              className="text-xs bg-background border border-input rounded-md px-2.5 py-1.5 focus:outline-hidden focus:ring-1 focus:ring-ring cursor-pointer"
            >
              <option value="active">{t.common.sortByActive}</option>
              <option value="blocks">{t.common.sortByBlocks}</option>
            </select>
          </div>
        </div>

        {/* Responsive Table */}
        <div className="overflow-x-auto">
          <table className="w-full text-left text-xs">
            <thead className="bg-muted/50 border-b border-border text-muted-foreground uppercase font-semibold">
              <tr>
                <th className="px-4 py-3">Worker ID</th>
                <th className="px-4 py-3">{t.common.status}</th>
                <th className="px-4 py-3">{t.common.engine}</th>
                <th className="px-4 py-3">{t.common.role}</th>
                <th className="px-4 py-3">{t.common.model}</th>
                <th className="px-4 py-3">{t.common.activeRequests}</th>
                <th className="px-4 py-3">Last Seq</th>
                <th className="px-4 py-3">HTTP / ZMQ Endpoint</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {filteredWorkers.length === 0 ? (
                <tr>
                  <td colSpan={8} className="px-4 py-6 text-center text-muted-foreground">
                    {loading ? t.cluster.loading : t.cluster.noWorkers}
                  </td>
                </tr>
              ) : (
                filteredWorkers.map((worker) => (
                  <tr key={worker.id} className="hover:bg-muted/30 transition-colors">
                    <td className="px-4 py-3 font-semibold text-foreground">{worker.id}</td>
                    <td className="px-4 py-3">{getStatusBadge(worker.status)}</td>
                    <td className="px-4 py-3">
                      <span className="px-2 py-0.5 rounded text-[11px] font-mono uppercase bg-primary/10 text-primary font-medium">
                        {worker.engine}
                      </span>
                    </td>
                    <td className="px-4 py-3 text-muted-foreground capitalize">{worker.role}</td>
                    <td className="px-4 py-3 font-mono text-[11px] text-foreground">{worker.model}</td>
                    <td className="px-4 py-3">
                      <span className={cn('font-semibold', worker.activeRequests > 5 ? 'text-warning' : 'text-foreground')}>
                        {worker.activeRequests}
                      </span>
                    </td>
                    <td className="px-4 py-3 font-mono font-medium text-foreground">{worker.lastSeq}</td>
                    <td className="px-4 py-3 font-mono text-[11px] text-muted-foreground">
                      <div>{worker.httpEndpoint}</div>
                      {worker.zmqEndpoint && <div className="text-[10px] text-muted-foreground/80">{worker.zmqEndpoint}</div>}
                    </td>
                  </tr>
                ))
              )}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  )
}

function RoutingBar({
  label,
  value,
  total,
}: {
  label: string
  value: number
  total: number
}) {
  const pct = total > 0 ? Math.round((value / total) * 100) : 0
  return (
    <div className="min-h-7">
      <div className="flex items-center justify-between text-xs mb-1">
        <span className="text-muted-foreground">{label}</span>
        <span className="font-semibold tabular-nums text-foreground">
          {value}
          <span className="text-muted-foreground font-normal ml-1.5">{pct}%</span>
        </span>
      </div>
      <div className="h-1.5 rounded-full bg-muted overflow-hidden">
        <div
          className="h-full rounded-full bg-primary/70"
          style={{ width: `${pct}%` }}
        />
      </div>
    </div>
  )
}
