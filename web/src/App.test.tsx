import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
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
    vi.unstubAllGlobals()
  })

  describe('export', () => {
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

      const clickSpy = vi
        .spyOn(HTMLAnchorElement.prototype, 'click')
        .mockImplementation(() => {})

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
      expect(screen.queryByRole('alert')).not.toBeInTheDocument()
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

  describe('import', () => {
    function makeFile(contents: string): File {
      return new File([contents], 'export.json', { type: 'application/json' })
    }

    it('POSTs file contents and reports the imported count', async () => {
      const fetchMock = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        json: () => Promise.resolve({ imported: 3 }),
      } as Response)
      vi.stubGlobal('fetch', fetchMock)

      render(<App />)
      const file = makeFile('[{"x":1}]')
      const input = screen.getByTestId('import-file-input') as HTMLInputElement
      await userEvent.upload(input, file)

      await waitFor(() => {
        expect(fetchMock).toHaveBeenCalledTimes(1)
      })
      const [url, init] = fetchMock.mock.calls[0]
      expect(url).toBe('/api/messages/import')
      expect(init.method).toBe('POST')
      expect(init.headers['Content-Type']).toBe('application/json')
      expect(init.body).toBe('[{"x":1}]')

      expect(await screen.findByRole('status')).toHaveTextContent(
        /Imported 3 messages/i,
      )
    })

    it('surfaces the server error message on conflict', async () => {
      const fetchMock = vi.fn().mockResolvedValue({
        ok: false,
        status: 409,
        json: () =>
          Promise.resolve({ error: 'import aborted: collides with existing' }),
      } as Response)
      vi.stubGlobal('fetch', fetchMock)

      render(<App />)
      const file = makeFile('[]')
      const input = screen.getByTestId('import-file-input') as HTMLInputElement
      await userEvent.upload(input, file)

      expect(await screen.findByRole('alert')).toHaveTextContent(/collides/)
    })

    it('imports a file dropped onto the drop zone', async () => {
      const fetchMock = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        json: () => Promise.resolve({ imported: 1 }),
      } as Response)
      vi.stubGlobal('fetch', fetchMock)

      render(<App />)
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
