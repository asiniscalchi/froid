# CONTRIBUTING.md

## Prerequisites

- Rust (edition 2024, see `Cargo.toml` for the exact toolchain)
- SQLite 3 development libraries
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

## Local CI Checks

Run these before pushing. They mirror the CI pipeline exactly:

```bash
cargo fmt --all --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

All four must pass with no errors or warnings before a branch is ready for review.

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
