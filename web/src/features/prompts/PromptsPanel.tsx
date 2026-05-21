import { useEffect, useRef, useState } from 'react'
import { CheckIcon } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/utils'
import {
  getPrompt,
  listPrompts,
  resetPrompt,
  savePrompt,
  type PromptDetail,
  type PromptListItem,
} from './api'

type Status =
  | { kind: 'idle' }
  | { kind: 'busy'; message: string }
  | { kind: 'success'; message: string }
  | { kind: 'error'; message: string }

function formatEditedAt(iso: string | null): string | null {
  if (!iso) return null
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return null
  return date.toLocaleString()
}

function PromptListSidebar({
  items,
  selectedKey,
  onSelect,
}: {
  items: PromptListItem[]
  selectedKey: string | null
  onSelect: (key: string) => void
}) {
  return (
    <aside className="md:w-64 md:shrink-0">
      <ScrollArea className="max-h-[60vh] md:max-h-[calc(100vh-12rem)]">
        <ul className="flex flex-col gap-1 pr-2">
          {items.map((item) => {
            const active = item.key === selectedKey
            return (
              <li key={item.key}>
                <Button
                  variant="ghost"
                  onClick={() => onSelect(item.key)}
                  aria-current={active ? 'page' : undefined}
                  className={cn(
                    'h-auto w-full justify-start gap-2 px-3 py-2 text-left',
                    active && 'bg-accent text-accent-foreground',
                  )}
                >
                  <span className="flex-1 truncate font-medium">
                    {item.label}
                  </span>
                  {item.is_customized && (
                    <Badge
                      variant="secondary"
                      className="font-normal"
                      aria-label="customized"
                    >
                      custom
                    </Badge>
                  )}
                </Button>
              </li>
            )
          })}
        </ul>
      </ScrollArea>
    </aside>
  )
}

function PromptsPanel() {
  const [items, setItems] = useState<PromptListItem[]>([])
  const [selectedKey, setSelectedKey] = useState<string | null>(null)
  const [detail, setDetail] = useState<PromptDetail | null>(null)
  const [draft, setDraft] = useState<string>('')
  const [status, setStatus] = useState<Status>({ kind: 'idle' })
  const [resetOpen, setResetOpen] = useState(false)
  const saveRef = useRef<() => void>(() => {})

  async function loadList(preserveSelection = true) {
    try {
      const data = await listPrompts()
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
      const data = await getPrompt(key)
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
      await savePrompt(selectedKey, draft)
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
    if (!selectedKey || !detail || !detail.is_customized) return
    setResetOpen(false)
    setStatus({ kind: 'busy', message: 'Resetting…' })
    try {
      await resetPrompt(selectedKey)
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
  const editedAt = detail ? formatEditedAt(detail.updated_at) : null

  useEffect(() => {
    saveRef.current = () => {
      if (dirty && !busy) void handleSave()
    }
  })

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 's') {
        e.preventDefault()
        saveRef.current()
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [])

  return (
    <div
      className="flex flex-col gap-4 md:flex-row md:gap-6"
      data-testid="prompts-panel"
    >
      <PromptListSidebar
        items={items}
        selectedKey={selectedKey}
        onSelect={setSelectedKey}
      />

      <section className="flex min-w-0 flex-1 flex-col gap-3">
        {detail ? (
          <>
            <header className="flex flex-wrap items-center gap-3">
              <h2 className="text-lg font-semibold tracking-tight">
                {detail.label}
              </h2>
              <Badge variant="outline" className="font-mono text-xs">
                {detail.current_version}
              </Badge>
              {detail.is_customized ? (
                <Badge variant="secondary">customized</Badge>
              ) : (
                <Badge variant="outline" className="text-muted-foreground">
                  default
                </Badge>
              )}
              {dirty && (
                <span className="text-xs font-medium text-primary">
                  unsaved changes
                </span>
              )}
              {editedAt && (
                <span className="ml-auto text-xs text-muted-foreground">
                  edited {editedAt}
                </span>
              )}
            </header>
            <Textarea
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              spellCheck={false}
              className="min-h-[60vh] resize-y font-mono text-xs leading-relaxed"
              data-testid="prompt-editor"
            />
            <div className="flex flex-wrap gap-2">
              <Button onClick={handleSave} disabled={busy || !dirty}>
                <CheckIcon aria-hidden />
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
                variant="ghost"
                onClick={() => setResetOpen(true)}
                disabled={busy || !detail.is_customized}
                className="text-destructive hover:bg-destructive/10 hover:text-destructive"
              >
                {status.kind === 'busy' && status.message === 'Resetting…'
                  ? 'Resetting…'
                  : 'Reset to default'}
              </Button>
              <span className="ml-auto self-center text-xs text-muted-foreground">
                <kbd className="rounded border border-border bg-muted px-1 font-mono">
                  ⌘
                </kbd>
                <span className="mx-1">+</span>
                <kbd className="rounded border border-border bg-muted px-1 font-mono">
                  S
                </kbd>{' '}
                to save
              </span>
            </div>
            <Dialog open={resetOpen} onOpenChange={setResetOpen}>
              <DialogContent>
                <DialogHeader>
                  <DialogTitle>Reset to default?</DialogTitle>
                  <DialogDescription>
                    This discards your customization of{' '}
                    <span className="font-medium text-foreground">
                      {detail.label}
                    </span>{' '}
                    and restores the bundled default. This cannot be undone.
                  </DialogDescription>
                </DialogHeader>
                <DialogFooter>
                  <DialogClose asChild>
                    <Button variant="outline">Cancel</Button>
                  </DialogClose>
                  <Button
                    variant="default"
                    onClick={handleReset}
                    className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                  >
                    Reset
                  </Button>
                </DialogFooter>
              </DialogContent>
            </Dialog>
            {status.kind === 'error' && (
              <p
                className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
                role="alert"
              >
                {status.message}
              </p>
            )}
            {status.kind === 'success' && (
              <p
                className="rounded-md border border-primary/30 bg-primary/10 px-3 py-2 text-sm text-foreground"
                role="status"
              >
                {status.message}
              </p>
            )}
          </>
        ) : (
          <p className="text-sm text-muted-foreground">Select a prompt to edit.</p>
        )}
      </section>
    </div>
  )
}

export default PromptsPanel
