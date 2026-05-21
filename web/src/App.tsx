import { useRef, useState, type DragEvent } from 'react'
import { Button } from '@/components/ui/button'
import Prompts from './Prompts'

function todayIsoDate(): string {
  return new Date().toISOString().slice(0, 10)
}

type Status =
  | { kind: 'idle' }
  | { kind: 'busy'; message: string }
  | { kind: 'success'; message: string }
  | { kind: 'error'; message: string }

function MessagesPanel() {
  const [status, setStatus] = useState<Status>({ kind: 'idle' })
  const [isDragging, setIsDragging] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const busy = status.kind === 'busy'

  async function handleExport() {
    setStatus({ kind: 'busy', message: 'Exporting…' })
    try {
      const response = await fetch('/api/messages/export')
      if (!response.ok) {
        throw new Error(`Export failed (${response.status})`)
      }
      const blob = await response.blob()
      const url = URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = url
      link.download = `froid-messages-${todayIsoDate()}.json`
      document.body.appendChild(link)
      link.click()
      link.remove()
      URL.revokeObjectURL(url)
      setStatus({ kind: 'idle' })
    } catch (err) {
      setStatus({
        kind: 'error',
        message: err instanceof Error ? err.message : 'Export failed',
      })
    }
  }

  async function handleImportFile(file: File) {
    setStatus({ kind: 'busy', message: 'Importing…' })
    try {
      const text = await file.text()
      const response = await fetch('/api/messages/import', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: text,
      })
      const payload = await response.json().catch(() => null)
      if (!response.ok) {
        const detail =
          payload && typeof payload.error === 'string'
            ? payload.error
            : `Import failed (${response.status})`
        throw new Error(detail)
      }
      const count = payload?.imported ?? 0
      setStatus({
        kind: 'success',
        message: `Imported ${count} message${count === 1 ? '' : 's'}.`,
      })
    } catch (err) {
      setStatus({
        kind: 'error',
        message: err instanceof Error ? err.message : 'Import failed',
      })
    }
  }

  function onFileChange(event: React.ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0]
    event.target.value = ''
    if (file) {
      void handleImportFile(file)
    }
  }

  function onDrop(event: DragEvent<HTMLDivElement>) {
    event.preventDefault()
    setIsDragging(false)
    const file = event.dataTransfer.files?.[0]
    if (file) {
      void handleImportFile(file)
    }
  }

  function onDragOver(event: DragEvent<HTMLDivElement>) {
    event.preventDefault()
    setIsDragging(true)
  }

  function onDragLeave() {
    setIsDragging(false)
  }

  return (
    <div className="flex flex-col items-center gap-6">
      <div className="flex gap-3">
        <Button onClick={handleExport} disabled={busy}>
          {status.kind === 'busy' && status.message === 'Exporting…'
            ? 'Exporting…'
            : 'Export raw messages'}
        </Button>
        <Button
          variant="secondary"
          onClick={() => fileInputRef.current?.click()}
          disabled={busy}
        >
          {status.kind === 'busy' && status.message === 'Importing…'
            ? 'Importing…'
            : 'Import messages'}
        </Button>
        <input
          ref={fileInputRef}
          type="file"
          accept="application/json,.json"
          className="hidden"
          onChange={onFileChange}
          data-testid="import-file-input"
        />
      </div>
      <div
        onDrop={onDrop}
        onDragOver={onDragOver}
        onDragLeave={onDragLeave}
        aria-label="Drop a JSON export to import"
        data-testid="import-dropzone"
        className={`flex h-24 w-80 items-center justify-center rounded-md border border-dashed text-sm transition-colors ${
          isDragging
            ? 'border-primary bg-primary/5 text-foreground'
            : 'border-muted-foreground/40 text-muted-foreground'
        }`}
      >
        Drop a JSON export here to import
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
  )
}

type Tab = 'messages' | 'prompts'

function App() {
  const [tab, setTab] = useState<Tab>('messages')

  return (
    <main className="flex min-h-screen flex-col items-center gap-6 bg-background px-6 py-10 text-foreground">
      <h1 className="text-3xl font-semibold tracking-tight">
        Hello from Froid
      </h1>
      <nav
        className="flex gap-2"
        role="tablist"
        aria-label="Dashboard sections"
      >
        <Button
          variant={tab === 'messages' ? 'default' : 'ghost'}
          size="sm"
          role="tab"
          aria-selected={tab === 'messages'}
          onClick={() => setTab('messages')}
        >
          Messages
        </Button>
        <Button
          variant={tab === 'prompts' ? 'default' : 'ghost'}
          size="sm"
          role="tab"
          aria-selected={tab === 'prompts'}
          onClick={() => setTab('prompts')}
        >
          Prompts
        </Button>
      </nav>
      {tab === 'messages' ? <MessagesPanel /> : <Prompts />}
    </main>
  )
}

export default App
