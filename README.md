# Froid

Froid is an AI-powered personal journaling backend. It captures your thoughts via a Telegram bot, enriches them with structured analysis and semantic embeddings, and delivers a daily reflection back to you.

## How it works

Send a message to your Telegram bot. Froid stores it immediately and returns a confirmation. In the background, two workers process each entry:

- **Extraction** — an LLM reads the entry and produces a structured document: emotions (with intensity and confidence), behaviors (with valence), psychological needs (with status), and possible patterns. All inference is explicit about uncertainty and never overstates what a single note can support.
- **Embedding** — the entry is vectorised for semantic similarity search, so you can query your journal by meaning rather than keywords.

At the end of the day, a review worker synthesises all of that day's raw notes and their structured extractions into a concise reflection delivered via Telegram.

Once a week (Monday by default), a weekly review worker synthesises the previous ISO week's daily reviews and their structured signals into a single reflection covering Monday through Sunday, and delivers it via Telegram. Run `/week_review` in the chat to request the most recent completed weekly review on demand.

## Running with Docker

A pre-built image is published to the GitHub Container Registry on every push to `main`.

Create an `.env` file:

```env
TELEGRAM_BOT_TOKEN=your-token-here
OPENAI_API_KEY=your-key-here        # required if enabling workers

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

See the [project wiki](https://github.com/asiniscalchi/froid/wiki) for the full list of configuration options.

## Exposing tools over MCP

Run `cargo run -- mcp` to expose the analyzer's read-only tools (recent entries, text and semantic search, daily and weekly reviews, signals) over the MCP Streamable HTTP transport at `http://127.0.0.1:8080/mcp`. Froid is a single-user journal, so MCP requests use the local journal without a user-id argument. See [CONTRIBUTING.md](CONTRIBUTING.md#running-locally) for details.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup, build instructions, and project conventions.

## License

This project is licensed under the GNU Affero General Public License v3.0 or later.

SPDX-License-Identifier: AGPL-3.0-or-later — see [LICENSE](LICENSE).
