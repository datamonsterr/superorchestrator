import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { toast } from 'sonner'
import { describe, expect, it, vi } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import { createQueryClient } from '@/lib/query-client'
import { RunWorkflowDialog } from './RunWorkflowDialog'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }), Toaster: () => null }))

describe('RunWorkflowDialog', () => {
  it('toasts through the app query client when the run cannot start (501)', async () => {
    server.use(http.post('/api/v1/workflows/:name/run', () => HttpResponse.json({ error: { code: 'not_implemented', message: 'workflows are not available' } }, { status: 501 })))
    renderWithProviders(<RunWorkflowDialog name="ship-feature" onClose={() => {}} />, { queryClient: createQueryClient() })
    await userEvent.click(await screen.findByRole('button', { name: 'Run' }))
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith('Action failed', { description: 'workflows are not available' }))
    expect(screen.getByRole('dialog')).toBeInTheDocument()
  })
})
