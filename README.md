# Froid

Froid is an AI-powered personal journaling backend. It captures your thoughts via a Telegram bot, enriches them with structured analysis and semantic embeddings, and delivers a daily reflection back to you.

## How it works

Send a message to your Telegram bot. Froid stores it immediately and returns a confirmation. In the background, two workers process each entry:

- **Extraction** — an LLM reads the entry and produces a structured document: emotions (with intensity and confidence), behaviors (with valence), psychological needs (with status), and possible patterns. All inference is explicit about uncertainty and never overstates what a single note can support.
- **Embedding** — the entry is vectorised for semantic similarity search, so you can query your journal by meaning rather than keywords.

At the end of the day, a review worker synthesises all of that day's raw notes and their structured extractions into a concise reflection delivered via Telegram.

## Requirements

- Rust (edition 2024)
- SQLite 3 + development libraries (`libsqlite3-dev` on Debian/Ubuntu)
- A Telegram bot token
- An OpenAI API key (only required if embedding or extraction workers are enabled)

## Getting started

```bash
# Clone the repo
git clone https://github.com/asiniscalchi/froid.git
cd froid

# Configure environment
cp .env.example .env
# Fill in TELEGRAM_BOT_TOKEN (and OPENAI_API_KEY if enabling workers)

# Run
cargo run -- serve
```

Database migrations are applied automatically on startup.

## Configuration

All runtime configuration is read from environment variables (or a `.env` file in the working directory). CLI flags mirror the same names.

| Variable | Default | Description |
|---|---|---|
| `TELEGRAM_BOT_TOKEN` | — | **Required.** Telegram bot credentials. |
| `OPENAI_API_KEY` | — | Required when embedding or extraction workers are enabled. |
| `DATA_DIR` | `data` | Directory for persistent data. |
| `DATABASE_FILE` | `froid.sqlite3` | SQLite database path. |
| `RUST_LOG` | `info` | Log level filter (e.g. `debug`, `froid=trace`). |
| `FROID_EMBEDDING_WORKER_ENABLED` | `false` | Enable the embedding reconciliation worker. |
| `FROID_EMBEDDING_WORKER_BATCH_SIZE` | `20` | Entries processed per cycle. |
| `FROID_EMBEDDING_WORKER_INTERVAL_SECONDS` | `300` | Polling interval in seconds. |
| `FROID_EXTRACTION_WORKER_ENABLED` | `false` | Enable the extraction reconciliation worker. |
| `FROID_EXTRACTION_WORKER_BATCH_SIZE` | `20` | Entries processed per cycle. |
| `FROID_EXTRACTION_WORKER_INTERVAL_SECONDS` | `300` | Polling interval in seconds. |
| `FROID_DAILY_REVIEW_DELIVERY_ENABLED` | `false` | Enable the daily review delivery worker. |
| `FROID_DAILY_REVIEW_DELIVERY_INTERVAL_SECONDS` | — | Polling interval in seconds. |

## Docker

A pre-built image is published to the GitHub Container Registry on every push to `main`:

```bash
docker pull ghcr.io/asiniscalchi/froid:latest
```

To build locally:

```bash
docker build --build-arg FROID_VERSION=$(git rev-parse --short HEAD) -t froid .
docker run --env-file .env froid serve
```

## Development

```bash
# Check formatting
cargo fmt --all --check

# Compile
cargo check --locked --all-targets

# Lint
cargo clippy --locked --all-targets -- -D warnings

# Test
cargo test --locked --all-targets
```

All four checks must pass before pushing. They mirror the CI pipeline exactly.

See [CONTRIBUTING.md](CONTRIBUTING.md) for branch conventions, project structure, and architecture notes.

## Documentation

The [project wiki](https://github.com/asiniscalchi/froid/wiki) covers system design, the information model, database schema, the entry processing pipeline, and worker configuration.

## License

Apache 2.0 — see [LICENSE](LICENSE).
