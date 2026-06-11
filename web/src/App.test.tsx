import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import App from './App'

type RouteHandler = (init?: RequestInit) => Partial<Response>

function jsonResponse(payload: unknown, status = 200): Partial<Response> {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(payload),
  }
}

/** Stub fetch with default handlers for the panels that load on mount. */
function stubFetch(routes: Record<string, RouteHandler> = {}) {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    for (const [prefix, handler] of Object.entries(routes)) {
      if (url.startsWith(prefix)) {
        return Promise.resolve(handler(init) as Response)
      }
    }
    if (url.startsWith('/api/entries')) {
      return Promise.resolve(jsonResponse({ entries: [] }) as Response)
    }
    if (url.startsWith('/api/reviews/')) {
      return Promise.resolve(jsonResponse({ reviews: [] }) as Response)
    }
    if (url.startsWith('/api/prompts')) {
      return Promise.resolve(jsonResponse([]) as Response)
    }
    return Promise.resolve(jsonResponse(null, 404) as Response)
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

async function openMessagesTab() {
  await userEvent.click(screen.getByRole('tab', { name: /messages/i }))
}

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
    window.localStorage.clear()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  describe('tabs', () => {
    it('shows the journal panel by default', async () => {
      stubFetch()
      render(<App />)
      expect(await screen.findByTestId('journal-panel')).toBeInTheDocument()
    })
  })

  describe('token gate', () => {
    it('appears after a 401 and unlocks once a token is accepted', async () => {
      const fetchMock = stubFetch({
        '/api/entries': (init) => {
          const headers = (init?.headers ?? {}) as Record<string, string>
          return headers['Authorization'] === 'Bearer good-token'
            ? jsonResponse({ entries: [] })
            : jsonResponse(null, 401)
        },
      })
      render(<App />)

      expect(await screen.findByTestId('token-gate')).toBeInTheDocument()

      await userEvent.type(
        screen.getByLabelText(/bearer token/i),
        'good-token',
      )
      await userEvent.click(screen.getByRole('button', { name: /connect/i }))

      expect(await screen.findByTestId('journal-panel')).toBeInTheDocument()
      const verification = fetchMock.mock.calls.find(
        ([url]) => url === '/api/prompts',
      )
      expect(verification).toBeDefined()
    })

    it('shows an error for a rejected token', async () => {
      stubFetch({
        '/api/entries': () => jsonResponse(null, 401),
        '/api/prompts': () => jsonResponse(null, 401),
      })
      render(<App />)

      expect(await screen.findByTestId('token-gate')).toBeInTheDocument()
      await userEvent.type(screen.getByLabelText(/bearer token/i), 'bad-token')
      await userEvent.click(screen.getByRole('button', { name: /connect/i }))

      expect(await screen.findByRole('alert')).toHaveTextContent(
        /not accepted/i,
      )
    })
  })

  describe('export', () => {
    it('renders the export button', async () => {
      stubFetch()
      render(<App />)
      await openMessagesTab()
      expect(
        screen.getByRole('button', { name: /export raw messages/i }),
      ).toBeInTheDocument()
    })

    it('downloads the export when the button is clicked', async () => {
      const blob = new Blob(['[]'], { type: 'application/json' })
      const fetchMock = stubFetch({
        '/api/messages/export': () => ({
          ok: true,
          status: 200,
          blob: () => Promise.resolve(blob),
        }),
      })

      const clickSpy = vi
        .spyOn(HTMLAnchorElement.prototype, 'click')
        .mockImplementation(() => {})

      render(<App />)
      await openMessagesTab()
      await userEvent.click(
        screen.getByRole('button', { name: /export raw messages/i }),
      )

      await waitFor(() => {
        expect(fetchMock).toHaveBeenCalledWith(
          '/api/messages/export',
          expect.anything(),
        )
      })
      expect(URL.createObjectURL).toHaveBeenCalledWith(blob)
      expect(clickSpy).toHaveBeenCalledTimes(1)
      expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:mock-url')
      expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    })

    it('shows an error message when the export request fails', async () => {
      stubFetch({
        '/api/messages/export': () => ({
          ok: false,
          status: 500,
          blob: () => Promise.resolve(new Blob()),
        }),
      })

      render(<App />)
      await openMessagesTab()
      await userEvent.click(
        screen.getByRole('button', { name: /export raw messages/i }),
      )

      expect(await screen.findByRole('alert')).toHaveTextContent(/500/)
      expect(URL.createObjectURL).not.toHaveBeenCalled()
    })
  })

  describe('import', () => {
    function makeFile(contents: string): File {
      return new File([contents], 'export.json', { type: 'application/json' })
    }

    it('POSTs file contents and reports the imported count', async () => {
      const fetchMock = stubFetch({
        '/api/messages/import': () => jsonResponse({ imported: 3 }),
      })

      render(<App />)
      await openMessagesTab()
      const file = makeFile('[{"x":1}]')
      const input = screen.getByTestId('import-file-input') as HTMLInputElement
      await userEvent.upload(input, file)

      await waitFor(() => {
        expect(
          fetchMock.mock.calls.some(
            ([url]) => url === '/api/messages/import',
          ),
        ).toBe(true)
      })
      const [url, init] = fetchMock.mock.calls.find(
        ([u]) => u === '/api/messages/import',
      )!
      expect(url).toBe('/api/messages/import')
      expect(init!.method).toBe('POST')
      expect(
        (init!.headers as Record<string, string>)['Content-Type'],
      ).toBe('application/json')
      expect(init!.body).toBe('[{"x":1}]')

      expect(await screen.findByRole('status')).toHaveTextContent(
        /Imported 3 messages/i,
      )
    })

    it('surfaces the server error message on conflict', async () => {
      stubFetch({
        '/api/messages/import': () =>
          jsonResponse({ error: 'import aborted: collides with existing' }, 409),
      })

      render(<App />)
      await openMessagesTab()
      const file = makeFile('[]')
      const input = screen.getByTestId('import-file-input') as HTMLInputElement
      await userEvent.upload(input, file)

      expect(await screen.findByRole('alert')).toHaveTextContent(/collides/)
    })

    it('imports a file dropped onto the drop zone', async () => {
      const fetchMock = stubFetch({
        '/api/messages/import': () => jsonResponse({ imported: 1 }),
      })

      render(<App />)
      await openMessagesTab()
      const dropzone = screen.getByTestId('import-dropzone')
      const file = makeFile('[{"x":1}]')
      fireEvent.drop(dropzone, {
        dataTransfer: { files: [file] },
      })

      expect(await screen.findByRole('status')).toHaveTextContent(
        /Imported 1 message\b/i,
      )
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/messages/import',
        expect.objectContaining({ method: 'POST' }),
      )
    })
  })
})
