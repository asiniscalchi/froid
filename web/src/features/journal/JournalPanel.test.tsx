import { afterEach, describe, expect, it, vi } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import JournalPanel from './JournalPanel'

function jsonResponse(payload: unknown, status = 200): Partial<Response> {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(payload),
    text: () => Promise.resolve(JSON.stringify(payload)),
  }
}

describe('JournalPanel', () => {
  afterEach(() => {
    window.localStorage.clear()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  it('lists recent entries', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() =>
        Promise.resolve(
          jsonResponse({
            entries: [
              {
                id: '01A',
                text: 'first note',
                received_at: '2026-06-01T10:00:00Z',
              },
            ],
          }) as Response,
        ),
      ),
    )

    render(<JournalPanel />)

    expect(await screen.findByText('first note')).toBeInTheDocument()
  })

  it('captures an entry and refreshes the list', async () => {
    const entries: Array<{ id: string; text: string; received_at: string }> =
      []
    const fetchMock = vi.fn((url: string, init?: RequestInit) => {
      if (url === '/api/messages' && init?.method === 'POST') {
        const { text } = JSON.parse(init.body as string) as { text: string }
        entries.unshift({
          id: `id-${entries.length}`,
          text,
          received_at: '2026-06-10T08:00:00Z',
        })
        return Promise.resolve(jsonResponse(entries[0], 201) as Response)
      }
      return Promise.resolve(jsonResponse({ entries }) as Response)
    })
    vi.stubGlobal('fetch', fetchMock)

    render(<JournalPanel />)

    await userEvent.type(
      screen.getByTestId('journal-capture-input'),
      'a new thought',
    )
    await userEvent.click(screen.getByRole('button', { name: /add entry/i }))

    expect(await screen.findByText('a new thought')).toBeInTheDocument()
    await waitFor(() => {
      expect(
        (screen.getByTestId('journal-capture-input') as HTMLTextAreaElement)
          .value,
      ).toBe('')
    })
  })

  it('rejects submitting a blank entry', async () => {
    const fetchMock = vi.fn(() =>
      Promise.resolve(jsonResponse({ entries: [] }) as Response),
    )
    vi.stubGlobal('fetch', fetchMock)

    render(<JournalPanel />)
    await waitFor(() => expect(fetchMock).toHaveBeenCalled())

    expect(screen.getByRole('button', { name: /add entry/i })).toBeDisabled()
  })
})
