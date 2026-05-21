import { useRef, useState, type DragEvent } from 'react'
import { DownloadIcon, UploadCloudIcon } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { cn } from '@/lib/utils'
import { exportMessages, importMessages } from './api'

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
  const exporting = status.kind === 'busy' && status.message === 'Exporting…'
  const importing = status.kind === 'busy' && status.message === 'Importing…'
  const busy = status.kind === 'busy'

  async function handleExport() {
    setStatus({ kind: 'busy', message: 'Exporting…' })
    try {
      const blob = await exportMessages()
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
      const { imported } = await importMessages(file)
      setStatus({
        kind: 'success',
        message: `Imported ${imported} message${imported === 1 ? '' : 's'}.`,
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
    if (file) void handleImportFile(file)
  }

  function onDrop(event: DragEvent<HTMLDivElement>) {
    event.preventDefault()
    setIsDragging(false)
    const file = event.dataTransfer.files?.[0]
    if (file) void handleImportFile(file)
  }

  function onDragOver(event: DragEvent<HTMLDivElement>) {
    event.preventDefault()
    setIsDragging(true)
  }

  function openFilePicker() {
    if (!busy) fileInputRef.current?.click()
  }

  return (
    <div className="grid gap-6 md:grid-cols-2">
      <Card>
        <CardHeader>
          <CardTitle>Export raw messages</CardTitle>
          <CardDescription>
            Download every message as a JSON file you can archive or
            re-import elsewhere.
          </CardDescription>
          <CardAction>
            <DownloadIcon className="size-5 text-muted-foreground" aria-hidden />
          </CardAction>
        </CardHeader>
        <CardContent>
          <Button onClick={handleExport} disabled={busy}>
            {exporting ? 'Exporting…' : 'Export raw messages'}
          </Button>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Import messages</CardTitle>
          <CardDescription>
            Drop a JSON export to restore messages. Duplicates are skipped.
          </CardDescription>
          <CardAction>
            <UploadCloudIcon
              className="size-5 text-muted-foreground"
              aria-hidden
            />
          </CardAction>
        </CardHeader>
        <CardContent>
          <div
            role="button"
            tabIndex={busy ? -1 : 0}
            onClick={openFilePicker}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault()
                openFilePicker()
              }
            }}
            onDrop={onDrop}
            onDragOver={onDragOver}
            onDragLeave={() => setIsDragging(false)}
            aria-label="Drop a JSON export to import, or click to choose a file"
            aria-disabled={busy}
            data-testid="import-dropzone"
            className={cn(
              'flex h-32 cursor-pointer flex-col items-center justify-center gap-1 rounded-md border-2 border-dashed text-sm transition-colors',
              'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
              isDragging
                ? 'border-primary bg-primary/5 text-foreground'
                : 'border-border text-muted-foreground hover:border-primary/60 hover:bg-accent/40',
              busy && 'pointer-events-none opacity-60',
            )}
          >
            <UploadCloudIcon className="size-5" aria-hidden />
            <span>
              {importing
                ? 'Importing…'
                : 'Drop a JSON export here or click to choose'}
            </span>
          </div>
          <input
            ref={fileInputRef}
            type="file"
            accept="application/json,.json"
            className="hidden"
            onChange={onFileChange}
            data-testid="import-file-input"
          />
        </CardContent>
      </Card>

      {status.kind === 'error' && (
        <p
          className="md:col-span-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
          role="alert"
        >
          {status.message}
        </p>
      )}
      {status.kind === 'success' && (
        <p
          className="md:col-span-2 rounded-md border border-primary/30 bg-primary/10 px-3 py-2 text-sm text-foreground"
          role="status"
        >
          {status.message}
        </p>
      )}
    </div>
  )
}

export default MessagesPanel
