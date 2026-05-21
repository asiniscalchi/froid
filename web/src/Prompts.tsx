import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'

type PromptListItem = {
  key: string
  label: string
  default_version: string
  is_customized: boolean
  updated_at: string | null
}

type PromptDetail = {
  key: string
  label: string
  default_version: string
  current_version: string
  default_text: string
  current_text: string
  is_customized: boolean
  updated_at: string | null
}

type Status =
  | { kind: 'idle' }
  | { kind: 'busy'; message: string }
  | { kind: 'success'; message: string }
  | { kind: 'error'; message: string }

function Prompts() {
  const [items, setItems] = useState<PromptListItem[]>([])
  const [selectedKey, setSelectedKey] = useState<string | null>(null)
  const [detail, setDetail] = useState<PromptDetail | null>(null)
  const [draft, setDraft] = useState<string>('')
  const [status, setStatus] = useState<Status>({ kind: 'idle' })

  async function loadList(preserveSelection = true) {
    try {
      const response = await fetch('/api/prompts')
      if (!response.ok) {
        throw new Error(`Failed to load prompts (${response.status})`)
      }
      const data: PromptListItem[] = await response.json()
      setItems(data)
      if (!preserveSelection && data.length > 0) {
        setSelectedKey(data[0].key)
      }
    } catch (err) {
      setStatus({
        kind: 'error',
        message: err instanceof Error ? err.message : 'Failed to load prompts',
      })
    }
  }

  async function loadDetail(key: string) {
    setStatus({ kind: 'busy', message: 'Loading…' })
    try {
      const response = await fetch(`/api/prompts/${encodeURIComponent(key)}`)
      if (!response.ok) {
        throw new Error(`Failed to load prompt (${response.status})`)
      }
      const data: PromptDetail = await response.json()
      setDetail(data)
      setDraft(data.current_text)
      setStatus({ kind: 'idle' })
    } catch (err) {
      setStatus({
        kind: 'error',
        message: err instanceof Error ? err.message : 'Failed to load prompt',
      })
    }
  }

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void loadList(false)
  }, [])

  useEffect(() => {
    if (selectedKey) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      void loadDetail(selectedKey)
    }
  }, [selectedKey])

  async function handleSave() {
    if (!selectedKey || !detail) return
    setStatus({ kind: 'busy', message: 'Saving…' })
    try {
      const response = await fetch(
        `/api/prompts/${encodeURIComponent(selectedKey)}`,
        {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ content: draft }),
        },
      )
      const payload = await response.json().catch(() => null)
      if (!response.ok) {
        const detailMsg =
          payload && typeof payload.error === 'string'
            ? payload.error
            : `Save failed (${response.status})`
        throw new Error(detailMsg)
      }
      await loadList()
      await loadDetail(selectedKey)
      setStatus({ kind: 'success', message: 'Saved.' })
    } catch (err) {
      setStatus({
        kind: 'error',
        message: err instanceof Error ? err.message : 'Save failed',
      })
    }
  }

  async function handleReset() {
    if (!selectedKey || !detail) return
    if (!detail.is_customized) {
      setStatus({ kind: 'idle' })
      return
    }
    if (
      !window.confirm(
        `Reset "${detail.label}" to the bundled default? This discards your customization.`,
      )
    ) {
      return
    }
    setStatus({ kind: 'busy', message: 'Resetting…' })
    try {
      const response = await fetch(
        `/api/prompts/${encodeURIComponent(selectedKey)}`,
        { method: 'DELETE' },
      )
      if (!response.ok) {
        throw new Error(`Reset failed (${response.status})`)
      }
      await loadList()
      await loadDetail(selectedKey)
      setStatus({ kind: 'success', message: 'Reset to default.' })
    } catch (err) {
      setStatus({
        kind: 'error',
        message: err instanceof Error ? err.message : 'Reset failed',
      })
    }
  }

  const dirty = detail !== null && draft !== detail.current_text
  const busy = status.kind === 'busy'

  return (
    <div
      className="flex w-full max-w-5xl flex-col gap-4"
      data-testid="prompts-panel"
    >
      <div className="flex flex-wrap gap-2">
        {items.map((item) => (
          <Button
            key={item.key}
            variant={item.key === selectedKey ? 'default' : 'outline'}
            size="sm"
            onClick={() => setSelectedKey(item.key)}
          >
            {item.label}
            {item.is_customized && (
              <span
                className="ml-1 text-xs opacity-70"
                aria-label="customized"
              >
                •
              </span>
            )}
          </Button>
        ))}
      </div>

      {detail && (
        <div className="flex flex-col gap-3">
          <div className="flex items-baseline justify-between gap-3 text-sm text-muted-foreground">
            <span>
              {detail.label} — version{' '}
              <code className="font-mono">{detail.current_version}</code>
            </span>
            {detail.updated_at && (
              <span>
                edited {new Date(detail.updated_at).toLocaleString()}
              </span>
            )}
          </div>
          <Textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            spellCheck={false}
            className="min-h-[400px] font-mono text-xs"
            data-testid="prompt-editor"
          />
          <div className="flex gap-2">
            <Button onClick={handleSave} disabled={busy || !dirty}>
              {status.kind === 'busy' && status.message === 'Saving…'
                ? 'Saving…'
                : 'Save'}
            </Button>
            <Button
              variant="outline"
              onClick={() => setDraft(detail.current_text)}
              disabled={busy || !dirty}
            >
              Discard changes
            </Button>
            <Button
              variant="secondary"
              onClick={handleReset}
              disabled={busy || !detail.is_customized}
            >
              {status.kind === 'busy' && status.message === 'Resetting…'
                ? 'Resetting…'
                : 'Reset to default'}
            </Button>
          </div>
          {status.kind === 'error' && (
            <p className="text-sm text-destructive" role="alert">
              {status.message}
            </p>
          )}
          {status.kind === 'success' && (
            <p className="text-sm text-foreground" role="status">
              {status.message}
            </p>
          )}
        </div>
      )}
    </div>
  )
}

export default Prompts
