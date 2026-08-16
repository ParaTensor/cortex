import React from 'react'
import { Link, useLocation } from 'react-router-dom'
import { Server, Activity, Network, Settings, Cpu } from 'lucide-react'
import { useI18n } from '@/lib/i18n'
import { cn } from '@/lib/utils'

export const AppSidebar: React.FC = () => {
  const location = useLocation()
  const { t } = useI18n()

  const navItems = [
    { path: '/', label: t.common.cluster, icon: Server },
    { path: '/workers', label: t.common.workers, icon: Cpu },
    { path: '/radix', label: t.common.radixTree, icon: Network },
    { path: '/metrics', label: t.common.dashboard, icon: Activity },
    { path: '/settings', label: t.common.settings, icon: Settings },
  ]

  return (
    <aside className="w-64 border-r border-sidebar-border bg-sidebar-background flex flex-col shrink-0 h-screen sticky top-0">
      {/* Brand Header */}
      <div className="h-14 px-5 flex items-center gap-3 border-b border-sidebar-border">
        <div className="w-8 h-8 rounded-md bg-primary flex items-center justify-center text-primary-foreground font-bold tracking-tight">
          CX
        </div>
        <div className="flex flex-col">
          <span className="font-semibold text-sm tracking-tight text-sidebar-foreground">Cortex Mesh</span>
          <span className="text-[11px] text-muted-foreground">KV & PD Gateway</span>
        </div>
      </div>

      {/* Navigation */}
      <nav className="flex-1 p-3 space-y-1 overflow-y-auto">
        {navItems.map((item) => {
          const Icon = item.icon
          const isActive = location.pathname === item.path
          return (
            <Link
              key={item.path}
              to={item.path}
              className={cn(
                'flex items-center gap-3 px-3 py-2 rounded-md text-sm font-medium transition-colors',
                isActive
                  ? 'bg-sidebar-accent text-sidebar-accent-foreground font-semibold'
                  : 'text-sidebar-foreground/80 hover:bg-sidebar-accent/50 hover:text-sidebar-foreground'
              )}
            >
              <Icon className="w-4 h-4 shrink-0 text-muted-foreground" />
              <span>{item.label}</span>
            </Link>
          )
        })}
      </nav>

      {/* Footer info */}
      <div className="p-4 border-t border-sidebar-border text-xs text-muted-foreground">
        <div>v0.1.0 (Rust 2024)</div>
        <div className="text-[11px] mt-0.5">Port 8000 (OpenAI Proxy)</div>
      </div>
    </aside>
  )
}
