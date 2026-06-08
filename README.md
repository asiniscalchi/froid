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
TELEGRAM_ALLOWED_USER_IDS=123456789,987654321    # optional: comma-separated list of allowed Telegram user IDs
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

Set `FROID_MCP_ENABLED=true` to expose the analyzer's read-only tools over the MCP Streamable HTTP transport at `http://127.0.0.1:8080/mcp`. The MCP server runs alongside the Telegram bot in the same process. Set `FROID_AUTH_TOKEN` to require an `Authorization: Bearer <token>` header on the HTTP listener (see [Authentication](#authentication)); when it is unset, the endpoints are unauthenticated, so restrict access at the network level — use the default loopback bind for local use, or a Docker internal network when running in Compose.

```bash
FROID_MCP_ENABLED=true cargo run -- serve
```

Available tools: `journal_get`, `journal_get_recent`, `journal_search_text`, `journal_search_semantic`, `daily_review_get`, `daily_review_get_range`, `weekly_review_get`, `weekly_review_get_range`, `signals_search`.

## Dashboard webapp

Set `FROID_DASHBOARD_ENABLED=true` to serve a small React webapp at `http://127.0.0.1:8080/`. The dashboard shares the HTTP listener with the MCP endpoint (`FROID_MCP_BIND`, default `127.0.0.1:8080`) and can be enabled independently of MCP. Assets are embedded into the release binary, so the Docker image carries everything it needs. The dashboard is protected by the same `FROID_AUTH_TOKEN` bearer check as MCP (see [Authentication](#authentication)); when no token is set, restrict access at the network level.

```bash
FROID_DASHBOARD_ENABLED=true cargo run -- serve
```

## Authentication

The HTTP listener shared by the MCP endpoint and the dashboard supports a single bearer token. Set `FROID_AUTH_TOKEN` to any secret string and every HTTP request must then carry a matching header:

```
Authorization: Bearer <your-token>
```

Requests without it receive `401 Unauthorized`. The same token guards both `/mcp` and the dashboard. MCP clients and scripts send the header natively; a browser opening the dashboard needs the header injected (e.g. via a reverse proxy or a header-setting extension).

When `FROID_AUTH_TOKEN` is unset, the endpoints are unauthenticated and Froid logs a warning at startup — in that case restrict access at the network level (loopback bind or a Docker internal network).

## Configuration

All options can be set via environment variables or the equivalent `--flag` CLI argument. Copy `.env.example` as a starting point.

### Core

| Variable | Default | Description |
|---|---|---|
| `TELEGRAM_BOT_TOKEN` | — | **Required.** Telegram bot credentials |
| `TELEGRAM_ALLOWED_USER_IDS` | _(all private chats)_ | Comma-separated list of allowed Telegram user/chat IDs |
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
| `FROID_MCP_BIND` | `127.0.0.1:8080` | Bind address (e.g. `0.0.0.0:8080` for Docker Compose) |
| `FROID_AUTH_TOKEN` | _(none)_ | Bearer token required on the HTTP listener (MCP and dashboard); unset means no authentication |

### Models

Override the OpenAI model used by each pipeline stage. Accepts any model name recognised by the OpenAI API.

| Variable | Default | Description |
|---|---|---|
| `FROID_EMBEDDING_MODEL` | `text-embedding-3-small` | Embedding model for journal entries and daily reviews |
| `FROID_ENTRY_EXTRACTION_MODEL` | `gpt-5-mini` | Model used for structured entry extraction |
| `FROID_REVIEW_MODEL` | `gpt-5-mini` | Model used for daily review generation |
| `FROID_SIGNAL_EXTRACTION_MODEL` | `gpt-5-mini` | Model used for daily review signal extraction |
| `FROID_WEEK_REVIEW_MODEL` | `gpt-5-mini` | Model used for weekly review generation |

### Prompts

Override the prompt file used by each pipeline stage. The version tag recorded in the database is derived automatically from the filename stem (e.g. `entry_extraction_v2` from `entry_extraction_v2.md`).

| Variable | Default |
|---|---|
| `FROID_ENTRY_EXTRACTION_PROMPT_PATH` | `prompts/entry_extraction_v1.md` |
| `FROID_REVIEW_PROMPT_PATH` | `prompts/daily_review_with_entry_extractions_v1.md` |
| `FROID_SIGNAL_EXTRACTION_PROMPT_PATH` | `prompts/daily_review_signal_extraction_v1.md` |
| `FROID_WEEK_REVIEW_PROMPT_PATH` | `prompts/weekly_review_v1.md` |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, build instructions, and project conventions.

## License

This project is licensed under the GNU Affero General Public License v3.0 or later.

SPDX-License-Identifier: AGPL-3.0-or-later — see [LICENSE](LICENSE).
