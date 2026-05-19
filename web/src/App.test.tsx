import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import App from './App'

describe('App', () => {
  const originalCreateObjectURL = URL.createObjectURL
  const originalRevokeObjectURL = URL.revokeObjectURL

  beforeEach(() => {
    URL.createObjectURL = vi.fn(() => 'blob:mock-url')
    URL.revokeObjectURL = vi.fn()
  })

  afterEach(() => {
    URL.createObjectURL = originalCreateObjectURL
    URL.revokeObjectURL = originalRevokeObjectURL
    vi.restoreAllMocks()
  })

  it('renders the export button', () => {
    render(<App />)
    expect(
      screen.getByRole('button', { name: /export raw messages/i }),
    ).toBeInTheDocument()
  })

  it('downloads the export when the button is clicked', async () => {
    const blob = new Blob(['[]'], { type: 'application/json' })
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      blob: () => Promise.resolve(blob),
    } as Response)
    vi.stubGlobal('fetch', fetchMock)

    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})

    render(<App />)
    await userEvent.click(
      screen.getByRole('button', { name: /export raw messages/i }),
    )

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith('/api/messages/export')
    })
    expect(URL.createObjectURL).toHaveBeenCalledWith(blob)
    expect(clickSpy).toHaveBeenCalledTimes(1)
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:mock-url')
    expect(
      screen.queryByRole('alert'),
    ).not.toBeInTheDocument()
  })

  it('shows an error message when the export request fails', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: false,
      status: 500,
      blob: () => Promise.resolve(new Blob()),
    } as Response)
    vi.stubGlobal('fetch', fetchMock)

    render(<App />)
    await userEvent.click(
      screen.getByRole('button', { name: /export raw messages/i }),
    )

    expect(await screen.findByRole('alert')).toHaveTextContent(/500/)
    expect(URL.createObjectURL).not.toHaveBeenCalled()
  })
})
