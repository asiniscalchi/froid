import { useEffect, useState } from 'react'
import { SendIcon } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Textarea } from '@/components/ui/textarea'
import { UnauthorizedError } from '@/lib/http'
import { captureEntry, listEntries, type JournalEntry } from './api'

type Status =
  | { kind: 'idle' }
  | { kind: 'busy' }
  | { kind: 'error'; message: string }

function formatReceivedAt(iso: string): string {
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return iso
  return date.toLocaleString()
}

function JournalPanel() {
  const [entries, setEntries] = useState<JournalEntry[]>([])
  const [draft, setDraft] = useState('')
  const [status, setStatus] = useState<Status>({ kind: 'idle' })

  async function load() {
    try {
      setEntries(await listEntries())
      setStatus({ kind: 'idle' })
    } catch (err) {
      if (err instanceof UnauthorizedError) return
      setStatus({
        kind: 'error',
        message: err instanceof Error ? err.message : 'Failed to load entries',
      })
    }
  }

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void load()
  }, [])

  async function handleCapture() {
    const text = draft.trim()
    if (!text) return
    setStatus({ kind: 'busy' })
    try {
      await captureEntry(text)
      setDraft('')
      await load()
    } catch (err) {
      if (err instanceof UnauthorizedError) return
      setStatus({
        kind: 'error',
        message: err instanceof Error ? err.message : 'Failed to save entry',
      })
    }
  }

  const busy = status.kind === 'busy'

  return (
    <div className="flex flex-col gap-6" data-testid="journal-panel">
      <section className="flex flex-col gap-3">
        <Textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="What's on your mind?"
          className="min-h-24 resize-y"
          data-testid="journal-capture-input"
        />
        <div className="flex items-center gap-3">
          <Button onClick={handleCapture} disabled={busy || !draft.trim()}>
            <SendIcon aria-hidden />
            {busy ? 'Saving…' : 'Add entry'}
          </Button>
          <span className="text-xs text-muted-foreground">
            Entries flow through the same analysis pipeline as Telegram
            messages.
          </span>
        </div>
      </section>

      {status.kind === 'error' && (
        <p
          className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
          role="alert"
        >
          {status.message}
        </p>
      )}

      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-semibold tracking-tight">
          Recent entries
        </h2>
        {entries.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No entries yet. Write one above or message your Telegram bot.
          </p>
        ) : (
          <ul className="flex flex-col gap-3">
            {entries.map((entry) => (
              <li key={entry.id}>
                <Card>
                  <CardContent className="flex flex-col gap-1 px-4">
                    <p className="text-sm whitespace-pre-wrap">{entry.text}</p>
                    <span className="text-xs text-muted-foreground">
                      {formatReceivedAt(entry.received_at)}
                    </span>
                  </CardContent>
                </Card>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  )
}

export default JournalPanel
