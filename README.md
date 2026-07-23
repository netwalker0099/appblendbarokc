# Blend Bar — Perfume Stand Intake App

Self-hosted stand-intake app: Rust/Axum API, Vue 3 frontend, Postgres, all behind
Caddy for automatic HTTPS. Squarespace is a downstream sink (Contacts + Orders APIs);
this app's own Postgres database is the source of truth for customers, preferences,
mixes, and orders.

## Services

- `db` — Postgres 16, persisted in the `pgdata` volume.
- `api` — Rust (Axum) backend, internal port 8080, not published directly.
- `caddy` — serves the built Vue static assets and reverse-proxies `/api/*` to `api`.
  Owns ports 80/443 and handles automatic TLS via Let's Encrypt for `DOMAIN`.

## Prerequisites on the VPS

- Docker Engine + the Docker Compose plugin (`docker compose version`).
- Ports 80 and 443 open in the firewall.
- A DNS A/AAAA record for the app's domain pointed at this VPS's public IP.
  **Caddy cannot issue a TLS certificate until this DNS record resolves** — until
  then it will retry the ACME challenge in the background and log failures; this is
  expected and not a build problem.

## Deploy

```bash
git clone <this repo> blendbar
cd blendbar
cp .env.example .env
# edit .env: set real POSTGRES_PASSWORD, OPERATOR_AUTH_SECRET, SQUARESPACE_API_KEY,
# and DOMAIN if it differs from app.theblendbarokc.com

docker compose up --build -d
docker compose ps        # all three services should report healthy
```

Verify end to end once DNS has propagated:

```bash
curl https://app.theblendbarokc.com/api/health
# {"status":"ok"}
```

Before DNS is live, you can still verify the containers build and talk to each other
over plain HTTP from inside the VPS:

```bash
docker compose exec caddy wget -qO- http://api:8080/api/health
```

## Local development

- `api`: `cd api && cargo run` (requires a local Postgres reachable at `DATABASE_URL`
  once the schema lands in a later milestone).
- `web`: `cd web && npm install && npm run dev` — Vite dev server. `/api/*` is proxied
  to `http://localhost:8080`; override with `API_ORIGIN=... npm run dev`.

## Operator UI

The UI is operator-driven: staff run it on the stand tablet. It needs a device token
before it will do anything.

```bash
docker compose exec api blendbar-api issue-device-token "Stand iPad"
```

Open the site, paste the token on the pairing screen, and it is kept in
`localStorage` until "Unpair" is used or the API rejects it (a 401 forces re-pairing).

- **Intake** — customer details, marketing consent, scent preferences, and the order
  (type, bottle size, status, amount). Custom mixes use the mix builder, capped at 8
  ingredients. Amounts are entered as the 3.4oz base formula; the 1.7oz and roller
  amounts are shown derived, never stored.
- **Lookup** — search customers by email, see their saved mixes and orders, and
  "Reorder" a mix to open intake with that customer and mix prefilled.
- **Admin** — manage the ingredient and scent catalogs (add, activate/deactivate)
  and view the Squarespace integration status: push backend (mock vs. live),
  whether the webhook receiver is enabled, sync job counts with a retry button,
  and recent inbound webhooks. Server-side secrets (`SQUARESPACE_API_KEY`,
  `SQUARESPACE_WEBHOOK_SECRET`) are set in `.env`, not from the UI.

Each submission carries a generated `Idempotency-Key` that is held steady across
retries, so a double-tap or a flaky connection cannot create two orders.

### Smoke test

`web/smoke.js` drives the whole flow in a real headless browser against a running
stack. It writes one customer and one order per run — see the header comment in that
file for the exact command.

## Squarespace sync

Squarespace is a downstream sink; this app's Postgres is the source of truth. Intake
enqueues contact + order pushes into a transactional `sync_outbox` (same transaction
as the writes they mirror, so nothing is lost to a crash), and a background worker
drains it with exponential-backoff retries, writing the returned Squarespace ids back
onto the customer/order rows.

The push backend is chosen at startup: with `SQUARESPACE_API_KEY` set it uses the
live HTTP client, otherwise an **in-process mock** (the mode the box runs in today —
no key yet). Wiring a real key in is the only change needed to go live; the HTTP
client's request shapes are untested against Squarespace until then.

- `GET /api/sync/status` — active backend, job counts by state, recent failures.
- `POST /api/sync/retry` — requeue every failed job (the manual "try again now").

### Inbound webhooks (payments coming back)

Payment is taken at the stand through the Squarespace POS, so Squarespace tells us
when an order is paid/fulfilled via a webhook. `POST /api/webhooks/squarespace` is a
public but **HMAC-verified** endpoint (Squarespace can't send an operator token): it
verifies the signature against `SQUARESPACE_WEBHOOK_SECRET`, dedups on the
notification id, fetches the order's authoritative state back from Squarespace, and
reconciles our matching order's status. **When `SQUARESPACE_WEBHOOK_SECRET` is unset
the receiver is disabled and returns 503** — it mutates order status, so it never
runs unauthenticated.

- `GET /api/webhooks/recent` — recent webhook activity (topic, status, match) for
  debugging reconciliation.

## Status

Milestones 1–7 are done and validated live on the VPS: scaffold + TLS, schema,
operator auth / CRUD / intake, the operator UI, the reorder endpoint, and the
Squarespace sync layer + webhook receiver (both validated end-to-end against the
mock). See `RESUME.md` for current state and open questions.
