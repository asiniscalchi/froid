# CONTRIBUTING.md

## Prerequisites

- Rust (edition 2024, see `Cargo.toml` for the exact toolchain)
- SQLite 3 development libraries
- Node.js 20+ and npm (required only to build the dashboard webapp in `web/`)
- A Telegram bot token (required to run the server)
- An OpenAI API key (required only when embedding or extraction workers are enabled)

## Setup

Copy the example environment file and fill in your values:

```bash
cp .env.example .env
```

See [README.md — Configuration](README.md#configuration) for the full list of environment variables and their defaults.

## Running Locally

```bash
cargo run -- serve
```

Database migrations are applied automatically on startup via `sqlx::migrate!()`.

## Dashboard webapp (`web/`)

The dashboard is a Vite + React + TypeScript app with Tailwind v4 and shadcn/ui. Assets are embedded into the release Rust binary via `rust-embed`; debug builds read from `web/dist/` on disk.

```bash
cd web
npm install
npm run dev      # local dev server with HMR
npm run build    # produces web/dist/ consumed by cargo build
```

`web/dist/` is gitignored except for a committed `index.html` placeholder used as a fallback when assets have not been built. Running `npm run build` locally overwrites that placeholder; revert with `git restore web/dist/index.html` before committing.

## Local CI Checks

Run these before pushing. They mirror the CI pipeline exactly:

```bash
cargo fmt --all --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

For changes under `web/`, also run from that directory:

```bash
npm run lint
npm test
npm run build
```

All checks must pass with no errors or warnings before a branch is ready for review.

## Branch Naming

| Prefix | Use for |
|---|---|
| `feature/<short-description>` | New functionality |
| `fix/<short-description>` | Bug fixes |
| `refactor/<short-description>` | Code restructuring |
| `chore/<short-description>` | Tooling, dependencies, config |
| `docs/<short-description>` | Documentation only |
| `test/<short-description>` | Tests only |

Never push directly to `main`.

## Project Structure

```
src/
├── main.rs              # CLI entry point (clap + dotenvy + tracing)
├── app.rs               # serve() — wires up DB, workers, and adapters
├── cli.rs               # ServeConfig and argument parsing
├── database.rs          # SQLite pool with sqlite-vec extension
├── handler.rs           # MessageHandler trait
├── messages.rs          # IncomingMessage / OutgoingMessage types
├── version.rs           # Version populated by build.rs
├── dashboard.rs        # Axum router serving the embedded web/ assets
├── adapters/
│   ├── telegram.rs      # Telegram adapter (teloxide)
│   └── mcp.rs           # MCP Streamable HTTP adapter
├── journal/
│   ├── service/         # Orchestrates all journal operations
│   ├── repository.rs    # DB access for entries
│   ├── entry.rs         # JournalEntry struct
│   ├── command.rs       # Command types
│   ├── search.rs        # Semantic search
│   ├── embedding/       # Vector embedding pipeline
│   ├── extraction/      # Structured extraction (OpenAI)
│   ├── review/          # Daily review generation (OpenAI) + signals
│   ├── week_review/     # Weekly review generation (OpenAI)
│   └── analyzer/        # Read-only query layer; tools exposed over MCP
└── workers/
    ├── embedding.rs     # EmbeddingReconciliationWorker
    ├── extraction.rs    # ExtractionReconciliationWorker
    ├── daily_review.rs  # DailyReviewDeliveryWorker
    ├── signals.rs       # SignalReconciliationWorker
    └── weekly_review.rs # WeeklyReviewDeliveryWorker
migrations/              # sqlx migrations (applied automatically)
prompts/                 # Versioned LLM prompt files
web/                     # Vite + React + Tailwind + shadcn dashboard
```

## Architecture Notes

- **Async throughout** — tokio multi-thread runtime; workers run as spawned background tasks.
- **SQLite + sqlite-vec** — vector similarity search runs in-process; no external vector store needed.
- **Adapter pattern** — `MessageHandler` trait decouples the core from the transport layer; Telegram and MCP are the two current adapters.
- **Configuration via env** — all runtime config is read from environment variables (or `.env`); CLI args forward to `ServeConfig`.
- **LLM integration** — embeddings and structured extraction use `rig-core` with the OpenAI provider; prompts live in `prompts/` and are versioned by filename.
- **Migrations** — add new `.sql` files under `migrations/` following the `000N_description.sql` naming convention; sqlx runs them in order.

## Adding a Prompt

Prompt files in `prompts/` are versioned by filename (e.g. `daily_review_v2.md`). When changing model behavior, create a new versioned file rather than editing an existing one, so deployed instances can be rolled back cleanly.

## Docker

The image is built as a multi-stage build. The runtime stage requires the `prompts/` directory to be present alongside the binary.

```bash
docker build --build-arg FROID_VERSION=$(git rev-parse --short HEAD) -t froid .
```
