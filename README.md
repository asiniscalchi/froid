# Froid

Froid is an AI-powered personal journaling backend. It captures your thoughts via a Telegram bot, enriches them with structured analysis and semantic embeddings, and delivers daily and weekly reflections back to you.

## How it works

Send a message to your Telegram bot. Froid stores it immediately and returns a confirmation. In the background, workers process each entry:

- **Extraction** — an LLM reads the entry and produces a structured document: emotions (with intensity and confidence), behaviors (with valence), psychological needs (with status), and possible patterns. All inference is explicit about uncertainty and never overstates what a single note can support.
- **Embedding** — the entry is vectorised for semantic similarity search, so you can query your journal by meaning rather than keywords.

At the end of the day, a review worker synthesises all of that day's raw notes and their structured extractions into a concise reflection delivered via Telegram.

Once a week (Monday by default), a weekly review worker synthesises the previous ISO week's daily reviews and their structured signals into a single reflection covering Monday through Sunday, and delivers it via Telegram. Run `/week_review` in the chat to request the most recent completed weekly review on demand.

## Running with Docker

A pre-built image is published to the GitHub Container Registry on every push to `main`.

Create an `.env` file (see [Configuration](#configuration) for the full variable reference):

```env
TELEGRAM_BOT_TOKEN=your-token-here
TELEGRAM_ALLOWED_USER_ID=123456789    # optional: restrict to one Telegram user
OPENAI_API_KEY=your-key-here

FROID_EMBEDDING_WORKER_ENABLED=true
FROID_DAILY_REVIEW_EMBEDDING_WORKER_ENABLED=true
FROID_EXTRACTION_WORKER_ENABLED=true
FROID_DAILY_REVIEW_DELIVERY_ENABLED=true
FROID_WEEK_REVIEW_WORKER_ENABLED=true
```

Then run:

```bash
docker run --env-file .env -v ./data:/app/data ghcr.io/asiniscalchi/froid:latest serve
```

## Exposing tools over MCP

Set `FROID_MCP_ENABLED=true` to expose the analyzer's read-only tools over the MCP Streamable HTTP transport at `http://127.0.0.1:8080/mcp`. The MCP server runs alongside the Telegram bot in the same process. Froid is a single-user journal; MCP bind addresses must be loopback.

```bash
FROID_MCP_ENABLED=true cargo run -- serve
```

Available tools: `journal_get`, `journal_get_recent`, `journal_search_text`, `journal_search_semantic`, `daily_review_get`, `daily_review_get_range`, `weekly_review_get`, `weekly_review_get_range`, `signals_search`.

## Configuration

All options can be set via environment variables or the equivalent `--flag` CLI argument. Copy `.env.example` as a starting point.

### Core

| Variable | Default | Description |
|---|---|---|
| `TELEGRAM_BOT_TOKEN` | — | **Required.** Telegram bot credentials |
| `TELEGRAM_ALLOWED_USER_ID` | _(all private chats)_ | Restrict incoming messages and review delivery to one Telegram user |
| `OPENAI_API_KEY` | — | Required when any worker or the MCP semantic search tool is enabled |
| `DATA_DIR` | `data` | Directory for persistent data |
| `DATABASE_FILE` | `froid.sqlite3` | SQLite database filename (resolved relative to `DATA_DIR`) |
| `RUST_LOG` | `info` | Log level filter (e.g. `debug`, `froid=trace`) |

### Workers

All workers are disabled by default and require `OPENAI_API_KEY`.

| Variable | Default | Description |
|---|---|---|
| `FROID_EMBEDDING_WORKER_ENABLED` | `false` | Enable journal entry embedding worker |
| `FROID_EMBEDDING_WORKER_BATCH_SIZE` | `20` | Entries processed per cycle |
| `FROID_EMBEDDING_WORKER_INTERVAL_SECONDS` | `300` | Polling interval (seconds) |
| `FROID_DAILY_REVIEW_EMBEDDING_WORKER_ENABLED` | `false` | Enable daily review embedding worker |
| `FROID_DAILY_REVIEW_EMBEDDING_WORKER_BATCH_SIZE` | `20` | Reviews processed per cycle |
| `FROID_DAILY_REVIEW_EMBEDDING_WORKER_INTERVAL_SECONDS` | `300` | Polling interval (seconds) |
| `FROID_EXTRACTION_WORKER_ENABLED` | `false` | Enable structured extraction worker |
| `FROID_EXTRACTION_WORKER_BATCH_SIZE` | `20` | Entries processed per cycle |
| `FROID_EXTRACTION_WORKER_INTERVAL_SECONDS` | `300` | Polling interval (seconds) |
| `FROID_DAILY_REVIEW_DELIVERY_ENABLED` | `false` | Enable daily review generation and delivery |
| `FROID_DAILY_REVIEW_DELIVERY_INTERVAL_SECONDS` | `300` | Polling interval (seconds) |
| `FROID_SIGNAL_WORKER_ENABLED` | `false` | Enable daily review signal extraction worker |
| `FROID_SIGNAL_WORKER_BATCH_SIZE` | `20` | Reviews processed per cycle |
| `FROID_SIGNAL_WORKER_INTERVAL_SECONDS` | `300` | Polling interval (seconds) |
| `FROID_WEEK_REVIEW_WORKER_ENABLED` | `false` | Enable weekly review generation and delivery |
| `FROID_WEEK_REVIEW_WORKER_INTERVAL_SECONDS` | `300` | Polling interval (seconds) |
| `FROID_WEEK_REVIEW_KICKOFF_DAY` | `Monday` | Weekday on which weekly reviews are generated |
| `FROID_WEEK_REVIEW_MIN_DAILY_REVIEWS` | `1` | Minimum completed daily reviews required before generating a weekly review |

### MCP Server

| Variable | Default | Description |
|---|---|---|
| `FROID_MCP_ENABLED` | `false` | Enable the MCP Streamable HTTP server |
| `FROID_MCP_BIND` | `127.0.0.1:8080` | Bind address; must be a loopback address |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, build instructions, and project conventions.

## License

This project is licensed under the GNU Affero General Public License v3.0 or later.

SPDX-License-Identifier: AGPL-3.0-or-later — see [LICENSE](LICENSE).
