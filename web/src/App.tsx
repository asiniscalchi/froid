import { SparklesIcon } from 'lucide-react'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import MessagesPanel from '@/features/messages/MessagesPanel'
import PromptsPanel from '@/features/prompts/PromptsPanel'

function App() {
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
        <Tabs defaultValue="messages" className="gap-6">
          <TabsList aria-label="Dashboard sections">
            <TabsTrigger value="messages">Messages</TabsTrigger>
            <TabsTrigger value="prompts">Prompts</TabsTrigger>
          </TabsList>
          <TabsContent value="messages">
            <MessagesPanel />
          </TabsContent>
          <TabsContent value="prompts">
            <PromptsPanel />
          </TabsContent>
        </Tabs>
      </main>
    </div>
  )
}

export default App
