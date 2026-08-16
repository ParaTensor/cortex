import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { ClusterOverview } from '../ClusterOverview'

describe('ClusterOverview Component', () => {
  it('renders cluster title and worker elements', () => {
    render(<ClusterOverview />)
    expect(screen.getByText('集群概览')).toBeDefined()
    expect(screen.getByText('sgl-worker-01')).toBeDefined()
    expect(screen.getByText('sgl-worker-02')).toBeDefined()
  })
})
