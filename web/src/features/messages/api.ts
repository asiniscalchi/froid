import { apiFetch } from '@/lib/http'

export type ImportResult = { imported: number }

export async function exportMessages(): Promise<Blob> {
  const response = await apiFetch('/api/messages/export')
  if (!response.ok) {
    throw new Error(`Export failed (${response.status})`)
  }
  return response.blob()
}

export async function importMessages(file: File): Promise<ImportResult> {
  const text = await file.text()
  const response = await apiFetch('/api/messages/import', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: text,
  })
  const payload = (await response.json().catch(() => null)) as
    | { imported?: number; error?: string }
    | null
  if (!response.ok) {
    const detail =
      payload && typeof payload.error === 'string'
        ? payload.error
        : `Import failed (${response.status})`
    throw new Error(detail)
  }
  return { imported: payload?.imported ?? 0 }
}
