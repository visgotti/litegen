# LiteGen Price API

A standalone service that serves **model pricing** for every provider LiteGen
supports — some scraped dynamically on a cron, some curated by hand. It is a
**platform service, fully decoupled from the open-source `litegen-core` proxy**
(different language, process, and database); the only link is a dev/CI coverage
check that reads `litegen-core`'s `models/*.yaml` to guarantee parity.

- **Stack:** NestJS + TypeScript, TypeORM + Postgres, OpenAPI/Swagger, OAuth2
  (client-credentials, self-issued JWT), rate limiting, Helmet.
- **Per-provider folders:** each provider declares a config (scraped vs manual,
  cron schedule, pricing URL), seed prices, and a scraper (real or stub).
- **Freshness model:** a failed scrape never destroys a price — it degrades
  `fresh → stale → failed` while serving the last-known-good value.

## Quick start

```bash
cp .env.example .env                 # set JWT_SECRET, DATABASE_URL, etc.
docker compose up -d db              # Postgres on :5433
npm install
npm run coverage:check               # verify every litegen model is covered
npm run start:dev                    # boots, seeds, serves on :4100
```

Open **http://localhost:4100/docs** for the Swagger UI.

## Endpoints

| Method | Path | Auth | Description |
| --- | --- | --- | --- |
| GET | `/v1/providers` | public | Providers with mode, cron, last-scraped, freshness |
| GET | `/v1/providers/:id` | public | Provider + all its models & prices |
| GET | `/v1/models` | public | Models + current prices (filter: `provider`, `mediaType`, `freshness`) |
| GET | `/v1/models/:provider/:name` | public | One model (e.g. `/v1/models/openai/dall-e-3`) |
| GET | `/v1/models/:provider/:name/history` | public | Price-change history |
| GET | `/v1/pricing` | public | Flat, filterable pricing table (primary consumer endpoint) |
| POST | `/oauth/token` | public | Client-credentials → JWT |
| POST | `/v1/admin/models/:provider/:name/price` | `pricing:admin` | Manual price upsert |
| PATCH | `/v1/admin/providers/:id` | `pricing:admin` | Change mode / cron / URL |
| POST | `/v1/admin/providers/:id/scrape` | `pricing:admin` | Trigger a scrape now |
| POST | `/v1/admin/clients` | `pricing:admin` | Create an OAuth client |
| GET | `/health`, `/health/ready` | public | Liveness / readiness |

### Getting a token

```bash
curl -s localhost:4100/oauth/token -H 'content-type: application/json' \
  -d '{"grant_type":"client_credentials","client_id":"bootstrap-admin","client_secret":"<secret>"}'
```

Reads (`GET`) are public and rate-limited; writes require the bearer token with
the `pricing:admin` scope.

## Pricing model

Each model has one or more **price components**: a `unit`
(`per_image`, `per_video`, `per_second`, `per_megapixel`, `per_request`), an
`amountUsd` per `unitAmount`, an optional `tier` (e.g. `{resolution:"1080p"}`),
plus `source` (`scraped`/`manual`/`fallback`) and `freshness`
(`fresh`/`stale`/`failed`) with `lastUpdatedAt` / `lastCheckedAt` timestamps.

## Providers

Add or change a provider in `src/providers/<id>/`:

- `*.config.ts` — `mode` (`scraped`/`manual`), `cronSchedule`, `pricingUrl`, notes.
- `*.seed.ts` — baseline prices for every model (the manual source of truth).
- `*.scraper.ts` — a `BaseProviderScraper` subclass (real) or a `StubScraper`.

Then register it in `src/providers/registry.ts`. Currently scraped: **openai**,
**fal** (image models; their video models and the other five providers are
curated `manual`). Scrapers parse the public pricing page with a fixture-tested
generic table parser and degrade gracefully if the page structure changes.

## Coverage guarantee

```bash
npm run coverage:check   # CI gate: fails if any litegen model lacks a config/seed/scraper
npm run coverage:fix     # scaffold stub folders for any missing providers
```

The canonical set is read from `LITEGEN_MODELS_DIR` (default `../../models`).

## Database & migrations

Dev uses `DB_SYNCHRONIZE=true`. **Production must set it `false` and use
migrations:**

```bash
npm run migration:generate -- src/database/migrations/Init
npm run migration:run
```

## Testing

```bash
npm test                 # unit tests (no DB needed)
npm run test:e2e         # e2e (needs Postgres; point TEST_DATABASE_URL at a scratch DB)
```
