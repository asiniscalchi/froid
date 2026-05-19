import { Button } from '@/components/ui/button'

function App() {
  return (
    <main className="flex min-h-screen items-center justify-center bg-background text-foreground">
      <div className="flex flex-col items-center gap-6">
        <h1 className="text-3xl font-semibold tracking-tight">
          Hello from Froid
        </h1>
        <p className="text-muted-foreground">
          Dashboard scaffold. More to come.
        </p>
        <Button>Get started</Button>
      </div>
    </main>
  )
}

export default App
