import { useEffect, useState } from 'react'
import { SparklesIcon } from 'lucide-react'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import TokenGate from '@/features/auth/TokenGate'
import JournalPanel from '@/features/journal/JournalPanel'
import MessagesPanel from '@/features/messages/MessagesPanel'
import PromptsPanel from '@/features/prompts/PromptsPanel'
import ReviewsPanel from '@/features/reviews/ReviewsPanel'
import { onUnauthorized } from '@/lib/http'

function App() {
  const [needsToken, setNeedsToken] = useState(false)
  // Bumped after a successful login to remount the panels so they refetch.
  const [sessionId, setSessionId] = useState(0)

  useEffect(() => onUnauthorized(() => setNeedsToken(true)), [])

  function handleAuthenticated() {
    setNeedsToken(false)
    setSessionId((id) => id + 1)
  }

  return (
    <div className="min-h-screen bg-background text-foreground">
      <header className="sticky top-0 z-10 border-b border-border/60 bg-background/80 backdrop-blur">
        <div className="mx-auto flex h-14 max-w-6xl items-center gap-2 px-6">
          <SparklesIcon className="size-5 text-primary" aria-hidden />
          <span className="text-base font-semibold tracking-tight">Froid</span>
          <span className="ml-1 text-sm text-muted-foreground">Dashboard</span>
        </div>
      </header>
      <main className="mx-auto w-full max-w-6xl px-6 py-8">
        {needsToken ? (
          <TokenGate onAuthenticated={handleAuthenticated} />
        ) : (
          <Tabs defaultValue="journal" className="gap-6" key={sessionId}>
            <TabsList aria-label="Dashboard sections">
              <TabsTrigger value="journal">Journal</TabsTrigger>
              <TabsTrigger value="reviews">Reviews</TabsTrigger>
              <TabsTrigger value="messages">Messages</TabsTrigger>
              <TabsTrigger value="prompts">Prompts</TabsTrigger>
            </TabsList>
            <TabsContent value="journal">
              <JournalPanel />
            </TabsContent>
            <TabsContent value="reviews">
              <ReviewsPanel />
            </TabsContent>
            <TabsContent value="messages">
              <MessagesPanel />
            </TabsContent>
            <TabsContent value="prompts">
              <PromptsPanel />
            </TabsContent>
          </Tabs>
        )}
      </main>
    </div>
  )
}

export default App
