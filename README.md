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
- `web`: `cd web && npm install && npm run dev` — Vite dev server; point it at a
  running `api` container for `/api/*` calls, or adjust `vite.config.js` to proxy to
  `http://localhost:8080` during development.

## Status

Milestone 1 (scaffold): Compose + Caddy + empty Rust service + Vue app, TLS and a
hello-world request proxied end to end via `GET /api/health`. No database schema,
auth, or Squarespace integration yet — see the build plan for subsequent milestones.
