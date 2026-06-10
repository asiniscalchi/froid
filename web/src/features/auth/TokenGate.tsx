import { useState } from 'react'
import { KeyRoundIcon } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { UnauthorizedError, apiFetch, setToken } from '@/lib/http'

/**
 * Full-screen prompt for the bearer token (FROID_AUTH_TOKEN or the user's
 * entry in FROID_AUTH_TOKENS). The token is verified against the API and
 * kept in localStorage for subsequent visits.
 */
function TokenGate({ onAuthenticated }: { onAuthenticated: () => void }) {
  const [value, setValue] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleSubmit(event: React.FormEvent) {
    event.preventDefault()
    const token = value.trim()
    if (!token) return
    setBusy(true)
    setError(null)
    setToken(token)
    try {
      const response = await apiFetch('/api/prompts')
      if (!response.ok) {
        throw new Error(`Verification failed (${response.status})`)
      }
      onAuthenticated()
    } catch (err) {
      setError(
        err instanceof UnauthorizedError
          ? 'That token was not accepted.'
          : err instanceof Error
            ? err.message
            : 'Verification failed',
      )
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      className="flex min-h-[60vh] items-center justify-center"
      data-testid="token-gate"
    >
      <Card className="w-full max-w-md">
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <KeyRoundIcon className="size-4" aria-hidden />
            Access token required
          </CardTitle>
        </CardHeader>
        <CardContent>
          <form className="flex flex-col gap-4" onSubmit={handleSubmit}>
            <div className="flex flex-col gap-2">
              <Label htmlFor="froid-token">Bearer token</Label>
              <Input
                id="froid-token"
                type="password"
                value={value}
                onChange={(e) => setValue(e.target.value)}
                placeholder="Paste your token"
                autoFocus
              />
              <p className="text-xs text-muted-foreground">
                The token configured via FROID_AUTH_TOKEN, or your personal
                entry in FROID_AUTH_TOKENS. It is stored only in this browser.
              </p>
            </div>
            {error && (
              <p
                className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
                role="alert"
              >
                {error}
              </p>
            )}
            <Button type="submit" disabled={busy || !value.trim()}>
              {busy ? 'Checking…' : 'Connect'}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  )
}

export default TokenGate
