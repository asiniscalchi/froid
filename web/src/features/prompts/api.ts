import { apiFetch } from '@/lib/http'

export type PromptListItem = {
  key: string
  label: string
  default_version: string
  is_customized: boolean
  updated_at: string | null
}

export type PromptDetail = {
  key: string
  label: string
  default_version: string
  current_version: string
  default_text: string
  current_text: string
  is_customized: boolean
  updated_at: string | null
}

export async function listPrompts(): Promise<PromptListItem[]> {
  const response = await apiFetch('/api/prompts')
  if (!response.ok) {
    throw new Error(`Failed to load prompts (${response.status})`)
  }
  return response.json()
}

export async function getPrompt(key: string): Promise<PromptDetail> {
  const response = await apiFetch(`/api/prompts/${encodeURIComponent(key)}`)
  if (!response.ok) {
    throw new Error(`Failed to load prompt (${response.status})`)
  }
  return response.json()
}

export async function savePrompt(
  key: string,
  content: string,
): Promise<PromptDetail> {
  const response = await apiFetch(`/api/prompts/${encodeURIComponent(key)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ content }),
  })
  const payload = (await response.json().catch(() => null)) as
    | (PromptDetail & { error?: string })
    | null
  if (!response.ok) {
    const detail =
      payload && typeof payload.error === 'string'
        ? payload.error
        : `Save failed (${response.status})`
    throw new Error(detail)
  }
  return payload as PromptDetail
}

export async function resetPrompt(key: string): Promise<void> {
  const response = await apiFetch(`/api/prompts/${encodeURIComponent(key)}`, {
    method: 'DELETE',
  })
  if (!response.ok) {
    throw new Error(`Reset failed (${response.status})`)
  }
}
