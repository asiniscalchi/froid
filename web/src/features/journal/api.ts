import { apiFetch } from '@/lib/http'

export type JournalEntry = {
  id: string
  text: string
  received_at: string
}

export async function listEntries(limit = 50): Promise<JournalEntry[]> {
  const response = await apiFetch(`/api/entries?limit=${limit}`)
  if (!response.ok) {
    throw new Error(`Failed to load entries (${response.status})`)
  }
  const payload = (await response.json()) as { entries: JournalEntry[] }
  return payload.entries
}

export async function captureEntry(text: string): Promise<JournalEntry> {
  const response = await apiFetch('/api/messages', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ text }),
  })
  if (!response.ok) {
    const detail = await response.text().catch(() => '')
    throw new Error(detail || `Failed to save entry (${response.status})`)
  }
  return response.json()
}
