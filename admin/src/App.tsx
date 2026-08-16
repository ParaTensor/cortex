import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom'
import { DashboardLayout } from './components/layout/DashboardLayout'
import { ClusterOverview } from './pages/ClusterOverview'

export function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<DashboardLayout />}>
          <Route path="/" element={<ClusterOverview />} />
          <Route path="/workers" element={<ClusterOverview />} />
          <Route path="/radix" element={<ClusterOverview />} />
          <Route path="/metrics" element={<ClusterOverview />} />
          <Route path="/settings" element={<ClusterOverview />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Route>
      </Routes>
    </BrowserRouter>
  )
}

export default App
