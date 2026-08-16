import React from 'react'
import { Outlet } from 'react-router-dom'
import { AppSidebar } from './AppSidebar'
import { SiteHeader } from './SiteHeader'

export const DashboardLayout: React.FC = () => {
  return (
    <div className="flex min-h-screen bg-background text-foreground">
      <AppSidebar />
      <div className="flex-1 flex flex-col min-w-0">
        <SiteHeader />
        <main className="flex-1 p-6 bg-muted/40 overflow-y-auto">
          <Outlet />
        </main>
      </div>
    </div>
  )
}
