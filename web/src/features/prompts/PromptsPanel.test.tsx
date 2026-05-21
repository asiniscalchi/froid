import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import Prompts from './PromptsPanel'

const listResponse = [
  {
    key: 'daily_review',
    label: 'Daily review',
    default_version: 'daily_review_with_entry_extractions_v1',
    is_customized: false,
    updated_at: null,
  },
  {
    key: 'weekly_review',
    label: 'Weekly review',
    default_version: 'weekly_review_v1',
    is_customized: true,
    updated_at: '2026-05-21T10:00:00Z',
  },
]

function detailResponse(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    key: 'daily_review',
    label: 'Daily review',
    default_version: 'daily_review_with_entry_extractions_v1',
    current_version: 'daily_review_with_entry_extractions_v1',
    default_text: 'Default body',
    current_text: 'Default body',
    is_customized: false,
    updated_at: null,
    ...overrides,
  }
}

describe('Prompts', () => {
  afterEach(() => {
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  describe('list + load', () => {
    beforeEach(() => {
      vi.stubGlobal(
        'fetch',
        vi.fn((url: string) => {
          if (url === '/api/prompts') {
            return Promise.resolve({
              ok: true,
              status: 200,
              json: () => Promise.resolve(listResponse),
            } as Response)
          }
          if (url.startsWith('/api/prompts/daily_review')) {
            return Promise.resolve({
              ok: true,
              status: 200,
              json: () => Promise.resolve(detailResponse()),
            } as Response)
          }
          return Promise.reject(new Error(`unexpected url ${url}`))
        }),
      )
    })

    it('renders one button per known prompt and loads the first one', async () => {
      render(<Prompts />)

      await waitFor(() => {
        expect(
          screen.getByRole('button', { name: /daily review/i }),
        ).toBeInTheDocument()
        expect(
          screen.getByRole('button', { name: /weekly review/i }),
        ).toBeInTheDocument()
      })

      const editor = await screen.findByTestId('prompt-editor')
      expect(editor).toHaveValue('Default body')
    })
  })

  describe('save', () => {
    it('sends PUT with new content and reports success', async () => {
      const fetchMock = vi.fn((url: string, init?: RequestInit) => {
        if (url === '/api/prompts' && (!init || init.method === undefined)) {
          return Promise.resolve({
            ok: true,
            status: 200,
            json: () => Promise.resolve(listResponse),
          } as Response)
        }
        if (
          url === '/api/prompts/daily_review' &&
          (!init || init.method === undefined)
        ) {
          return Promise.resolve({
            ok: true,
            status: 200,
            json: () => Promise.resolve(detailResponse()),
          } as Response)
        }
        if (
          url === '/api/prompts/daily_review' &&
          init?.method === 'PUT'
        ) {
          return Promise.resolve({
            ok: true,
            status: 200,
            json: () =>
              Promise.resolve(
                detailResponse({
                  current_text: 'New body',
                  is_customized: true,
                  current_version:
                    'daily_review_with_entry_extractions_v1-custom',
                  updated_at: '2026-05-21T11:00:00Z',
                }),
              ),
          } as Response)
        }
        return Promise.reject(new Error(`unexpected ${url} ${init?.method}`))
      })
      vi.stubGlobal('fetch', fetchMock)

      render(<Prompts />)
      const editor = await screen.findByTestId('prompt-editor')
      await userEvent.clear(editor)
      await userEvent.type(editor, 'New body')

      await userEvent.click(screen.getByRole('button', { name: /^save$/i }))

      const putCall = fetchMock.mock.calls.find(
        ([, init]) => init?.method === 'PUT',
      )
      expect(putCall).toBeDefined()
      expect(JSON.parse(String(putCall![1]!.body))).toEqual({
        content: 'New body',
      })
      expect(await screen.findByRole('status')).toHaveTextContent(/saved/i)
    })
  })

  describe('reset', () => {
    it('sends DELETE after the user confirms the dialog', async () => {
      const customizedList = [
        { ...listResponse[0], is_customized: true, updated_at: '2026-05-21T09:00:00Z' },
        listResponse[1],
      ]
      const fetchMock = vi.fn((url: string, init?: RequestInit) => {
        if (url === '/api/prompts' && (!init || init.method === undefined)) {
          return Promise.resolve({
            ok: true,
            status: 200,
            json: () => Promise.resolve(customizedList),
          } as Response)
        }
        if (
          url === '/api/prompts/daily_review' &&
          (!init || init.method === undefined)
        ) {
          return Promise.resolve({
            ok: true,
            status: 200,
            json: () =>
              Promise.resolve(
                detailResponse({ is_customized: true, current_text: 'Custom' }),
              ),
          } as Response)
        }
        if (
          url === '/api/prompts/daily_review' &&
          init?.method === 'DELETE'
        ) {
          return Promise.resolve({ ok: true, status: 204 } as Response)
        }
        return Promise.reject(new Error(`unexpected ${url} ${init?.method}`))
      })
      vi.stubGlobal('fetch', fetchMock)

      render(<Prompts />)
      await screen.findByTestId('prompt-editor')

      await userEvent.click(
        screen.getByRole('button', { name: /reset to default/i }),
      )
      const dialog = await screen.findByRole('dialog')
      await userEvent.click(
        within(dialog).getByRole('button', { name: /^reset$/i }),
      )

      await waitFor(() => {
        const del = fetchMock.mock.calls.find(
          ([, init]) => init?.method === 'DELETE',
        )
        expect(del).toBeDefined()
      })
    })
  })
})
