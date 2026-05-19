import { useState } from 'react'
import { Button } from '@/components/ui/button'

function todayIsoDate(): string {
  return new Date().toISOString().slice(0, 10)
}

function App() {
  const [isExporting, setIsExporting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleExport() {
    setIsExporting(true)
    setError(null)
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
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Export failed')
    } finally {
      setIsExporting(false)
    }
  }

  return (
    <main className="flex min-h-screen items-center justify-center bg-background text-foreground">
      <div className="flex flex-col items-center gap-6">
        <h1 className="text-3xl font-semibold tracking-tight">
          Hello from Froid
        </h1>
        <p className="text-muted-foreground">
          Dashboard scaffold. More to come.
        </p>
        <Button onClick={handleExport} disabled={isExporting}>
          {isExporting ? 'Exporting…' : 'Export raw messages'}
        </Button>
        {error && (
          <p className="text-sm text-destructive" role="alert">
            {error}
          </p>
        )}
      </div>
    </main>
  )
}

export default App
