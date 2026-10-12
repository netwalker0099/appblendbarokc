# Blend Bar — Perfume Stand Intake App

Self-hosted stand-intake app: Rust/Axum API, Vue 3 frontend, Postgres, all behind
Caddy for automatic HTTPS. **Square is the billing system** — in person a sale is
sent to a **Square Terminal** through Square's cloud Terminal API; online it is a
Square hosted payment link — so no card ever touches this app, while this app's own
Postgres remains the source of truth for customers, preferences, mixes, and orders.
See [Square billing](#square-billing).

> For a non-technical summary of status, risks, and what's needed to start taking
> payments, see **[EXECUTIVE_BRIEF.md](EXECUTIVE_BRIEF.md)**.

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

## Promoting the sandbox site

The customer site exists twice: `site/` is served at `sandbox.theblendbarokc.com`
(edit here), and `site-prod/` is served at `theblendbarokc.com` (`www` redirects
to it). Both use the same API and database. The production site changes only
when you copy the sandbox over it:

```bash
rsync -a --delete site/ site-prod/
docker compose up --build -d caddy
```

`CUSTOMER_SITE_URL` in `.env` sets which site the API links to in emails,
scent QR codes and the Square return page. Point it at `https://theblendbarokc.com`
once that domain's DNS is live, then `docker compose up -d api`.

> **The two are currently out of step.** `site/` was rebuilt around the design
> language described below; `site-prod/` is still the previous gold-on-cream
> design. Promoting with the `rsync` above is what brings production across, and
> it is a single deliberate step for exactly this reason.

### Design language of the customer site

`site/styles.css` is the whole system, and its header comment states the rules in
full. The short version: large tight-tracked sans headlines carrying each
section, a grey subhead under them, 980px pill buttons, 22–28px rounded
borderless cards, white and cream bands alternating instead of dividers, and one
quiet fade-and-rise on scroll. The serif survives on the brand wordmark only.

Two constraints worth knowing before editing:

- **No web fonts.** The CSP sets `font-src 'self'`, so the stack is
  `-apple-system` first — which resolves to SF Pro on Apple hardware and is most
  of where the look comes from — falling through Helvetica elsewhere.
- **One stylesheet serves every page**: the marketing home, `/portal`, `/book`,
  `/s/<id>` and `/thanks`. The component blocks at the bottom of the file dress
  markup generated at runtime by `share.js`, `portal/app.js` and `book.js`, so a
  class renamed there silently unstyles a page that is not the one being edited.

`site/home.js` adds the scroll reveal and the footer year. It is additive on
purpose: elements are hidden only if they are below the fold when the page loads,
so with JavaScript off or broken the whole page is simply visible.

## Local development

- `api`: `cd api && cargo run` (requires a local Postgres reachable at `DATABASE_URL`
  once the schema lands in a later milestone).
- `web`: `cd web && npm install && npm run dev` — Vite dev server. `/api/*` is proxied
  to `http://localhost:8080`; override with `API_ORIGIN=... npm run dev`.

## Operator UI

The UI is operator-driven: staff run it on the stand tablet. Every operator route
requires a signed-in **employee** with MFA completed.

Create the first admin, then sign in through the UI:

```bash
docker compose exec api blendbar-api create-admin someone@theblendbarokc.com
# prints a one-time temporary password
```

On first sign-in the app walks the employee through TOTP enrolment (scan the QR
with any authenticator app); after that, login is password + 6-digit code. The
session lives in an httpOnly cookie, not in `localStorage`, and admins manage
further accounts from **Admin → Team**.

#### The MFA trust window

Being made to fetch your phone every time the stand tablet's session lapses is
the friction staff actually feel, so a device that has completed MFA stays
trusted for a while: signing in again from it needs the **password only**. The
window is set in **Admin → Team → Sign-in security** (`settings.mfa_trust_hours`,
default **4 hours**, `0` disables it and asks for MFA every time, ceiling one
week).

What it does not relax:

- **The password is always required.** The trust cookie is consulted only after
  argon2 has verified the password, and authenticates nobody on its own.
- **A grant belongs to one employee.** The stand tablet is shared; trust issued
  to one person is ignored when anybody else signs in on it.
- **Enrolment is never skipped** — an employee who has not set up MFA still does.

A device stops being trusted early when the employee's password is changed or
reset, their MFA is reset, or their account is deactivated. Lowering the setting
applies immediately to grants already issued, because the window is re-checked
against the current value on every sign-in rather than only baked into the
cookie's expiry.

Signing out deliberately does *not* clear the trust — that is the case the
setting exists for. On a device that should not be remembered at all, set the
window to 0, or reset that employee's MFA to drop every grant they hold.

> **Superseded:** an earlier build paired tablets with a device token
> (`issue-device-token` + a paste-the-token screen). That model was replaced by
> employee accounts in Phase 2b. The CLI subcommand still exists but nothing
> checks the tokens it issues — `auth::require_operator_token` is dead code kept
> only until the subcommand is removed. Do not build against it.

- **Intake** — customer details, marketing consent, and the order (type, bottle
  size, status, amount). Custom mixes use the mix builder, capped at 8
  ingredients. Amounts are entered as the 3.4oz base formula; every other size is
  shown derived, never stored. **Every formula — house scent or custom blend —
  must total exactly 100 ml**, to 0.01 ml; the API refuses anything else and the
  builder shows how far off it is. Because each size is its volume over 100 ml,
  that one rule fills every bottle exactly.

  Six bottle sizes, largest first: **Vintage** (100 ml), **3.4 oz** (the base,
  100 ml), **1.7 oz** (50 ml), **1 oz** (30 ml), **Roller** (10 ml), and
  **Spray** (10 ml). The 1 oz was a 1.2 oz bottle until 2026-10-05 and keeps
  its stored code `oz1_2`. Two pairs hold the same volume and so
  take the same pour, differing only in the bottle and the price: Vintage/3.4 oz,
  and Roller/Spray (rollerball vs atomiser). Price is set per size on each scent
  and, for bespoke blends, globally in Admin. A size with no price set is simply
  not offered for sale.

  Scent preferences are no longer collected at intake — the stand flow is the
  customer, the bottles, and out. Preferences already on a customer record are
  untouched and still shown in Lookup.
- **Lookup** — search customers by email, see their saved mixes and orders, and
  "Reorder" a mix to open intake with that customer and mix prefilled.
- **Admin** — manage the ingredient and scent catalogs (add, activate/deactivate).
  Every ingredient is classified by perfumery **type** — Base, Top Note, or Heart
  Note — set on add and editable per row; the mix/scent builders group their
  ingredient picker by type. Each scent carries an editable **ingredient formula**
  (a house recipe), built with the same mix builder as custom mixes; set-perfume
  orders show that breakdown in intake and in a customer's order history. Also
  shows **Square billing** status (live vs. mock, webhook receiver, cart counts,
  money awaiting payment), **Square Terminals** (pairing, the default Terminal,
  Sandbox test devices), and recent inbound Square events. Server-side secrets (`SQUARE_ACCESS_TOKEN`,
  `SQUARE_WEBHOOK_SIGNATURE_KEY`, …) are set in `.env`, not from the UI.
- **Intake → payment.** Save intake now also builds the cart for what was just
  saved and shows **Charge $X on <Terminal>**; the customer taps or inserts on the
  Terminal and the screen flips to Paid on its own. A payment link / QR is offered
  as the fallback.
- **Checkout** — build a cart from a customer's unsold orders plus any ad-hoc lines
  (deposits, fees), then the same payment panel as intake. See
  [Square billing](#square-billing).

Each submission carries a generated `Idempotency-Key` that is held steady across
retries, so a double-tap or a flaky connection cannot create two orders.

### Smoke test

`web/smoke.js` drives the whole flow in a real headless browser against a running
stack. It writes one customer and one order per run — see the header comment in that
file for the exact command.

## Square billing

Square is the billing system. **This app never sees a card.** It assembles a cart,
hands it to Square as an order plus a hosted payment link, and the customer pays on
Square's own page — so card data never crosses the tablet, this server, or the shop's
network, and PCI scope stays with Square.

### The flow

```
intake                 checkout                  Square hosted page     webhook
──────                 ────────                  ──────────────────     ───────
order (status=lead) →  cart (one or more     →   customer scans QR  →   payment.updated
no money moved         orders + ad-hoc lines)    and pays               → cart=paid,
                       → Square order +                                   orders=paid
                         payment link
```

1. **Intake** creates orders exactly as before, `status = lead`. Nothing is owed yet.
2. **Checkout** (`/checkout` in the operator app) gathers one or more of that
   customer's unsold orders, plus any ad-hoc lines — an event deposit, a rush fee,
   the multi-day hotel line from the booking terms — into a **cart**.
3. `POST /api/carts/:id/checkout` pushes the cart to Square and gets back a hosted
   payment link. The screen shows it as a **QR code**: the customer scans with their
   own phone and pays there.
4. Square reports the result by webhook; the cart flips to `paid` and its orders with
   it. The screen updates on its own.

A cart is the unit of a checkout: one cart = one Square order = one payment. An order
can be claimed by at most one cart at a time (a unique index enforces it), so the same
blend can never be billed twice.

Cancelling a cart — by hand, or automatically after 24h unpaid — releases its orders
so they can be sold again **and voids the payment link at Square**. Both halves are
required: releasing without voiding would leave a live link that could still be paid
against blends since sold on another cart, and Square would be holding money nothing
here could explain.

Money is stored in **integer cents** on carts, matching Square, which never accepts
decimals. The conversion from the decimal prices operators type lives in
`api/src/square/money.rs` and is unit-tested — that is the one place a rounding bug
would silently charge the wrong amount without ever throwing.

### Buying from a share link (public checkout)

`POST /api/public/checkout` lets someone who was sent a `/s/<scent-id>` link buy
that scent without an account. It is the only endpoint in the app where an
anonymous caller can set money in motion, so it is deliberately narrow:

- **The price is read from the database**, keyed on the requested size — never
  taken from the request. A caller chooses *what* to buy, never what to pay.
- **An existing customer row is never modified.** Buying with someone else's email
  must not let a stranger rewrite their name or flip their marketing consent, so
  an existing record is used exactly as-is. New records are created with
  `marketing_consent = false` — a purchase is not consent.
- **Rate limited** to 10 attempts per IP per 5 minutes, keyed on the *rightmost*
  `X-Forwarded-For` entry (the one Caddy wrote; the leftmost is client-forgeable).
- **Refused with 503 unless Square is in production** (`SQUARE_ENV=production`).
  On the mock or on **sandbox** it refuses: a mock link is a dead URL, and a
  sandbox link takes a "payment" that moves no money, so neither may be handed
  to a real customer. Staff checkout still works against sandbox — that is how
  billing gets tested. It switches on by itself at go-live; there is no
  separate flag to remember. The share page then
  shows a "message us to order" fallback instead of a broken button.
- Square's error text is never returned to an anonymous caller — it can carry
  account and configuration detail.

Orders created this way carry `external_ref = 'public_share'` and a cart note, so
staff can see the blend has to be **made up and shipped/collected**, not handed
over at the bar. If such a checkout is abandoned, expiry deletes the speculative
order rather than leaving a phantom on that customer's account — orders taken at
the bar are left alone, because those blends physically exist.

After payment Square returns the buyer to `/thanks`.

### Paying on a Square Terminal

```
Save intake → cart → "Charge on Terminal"
                     → POST /v2/orders               (Square order for the cart)
                     → POST /v2/terminals/checkouts  (Square relays it to the Terminal)
customer taps / inserts on the Terminal
                     → payment on that order
                     → cart = paid, by whichever comes first:
                         payment.updated / terminal.checkout.updated webhook, or
                         the screen polling GET /api/carts/:id/terminal every 2 s
```

- **Any browser.** The request goes from this server to Square's cloud and from
  there to the Terminal over Wi-Fi/ethernet. Nothing is sent to the browser, there
  is no app-switch and no Bluetooth, so a laptop works exactly like an iPad. It
  needs a **Square Terminal**; a Square Reader (the "puck") can't be driven this
  way — it only works inside Square's own POS app or a native app built with
  Square's Mobile SDK.
- **Settlement is shared.** A Terminal payment carries the Square order id, so it
  lands in the same `billing::apply_payment` as a payment-link sale. The poll
  makes it work before webhooks are configured; the webhook makes it work when
  nobody is watching the screen.
- **Double-press safe.** While a checkout is on the device, "Charge" returns it
  instead of sending another. Idempotency keys are derived from the cart, so a
  retried request after a timeout can't queue a second checkout.
- **Cancel.** "Cancel on the Terminal" takes it off the device; the cart stays so
  it can be re-sent (same Square order) or canceled. Canceling the *sale* cancels
  on the Terminal **first** and refuses if Square says it's too late — the
  customer may be paying at that moment.
- **One method per cart.** Each path creates its own Square order and a cart has
  one `square_order_id`, so a cart with a payment link must be canceled before it
  can go to a Terminal, and vice versa.
- **Tips** entered on the Terminal stay in Square: the cart records the sale
  amount (`amount_money`), not the tip.
- **Pairing** (Admin → Billing → Square Terminals): the app asks Square for a
  device code; on the Terminal, *Sign in → Use a device code*. The panel polls (and
  the `device.code.paired` webhook also lands) until it's paired. The first paired
  Terminal is the default. In Sandbox, Square's published test device ids are
  added by id instead — they approve, cancel, time out or go offline on demand.

Routes: `POST/GET /api/carts/:id/terminal`, `POST /api/carts/:id/terminal/cancel`,
`GET/POST /api/square/terminals`, `POST /api/square/terminals/pair`,
`GET/PATCH/DELETE /api/square/terminals/:id`. Code: `api/src/routes/terminal.rs`,
`web/src/components/PayPanel.vue`, `TerminalManager.vue`.

### Square permissions

A **personal access token** (`SQUARE_ACCESS_TOKEN`) carries every permission on
the account. **Connect Square** (OAuth) lets the app hold only what it uses — the
set below, which is exactly what `square::oauth::SCOPES` asks for:

| Permission | Used for |
|---|---|
| `ORDERS_WRITE` | creating the Square order for a cart; payment links |
| `ORDERS_READ` | reading an order back ("Check Square"); payment links |
| `PAYMENTS_WRITE` | sending and canceling Terminal checkouts; payment links |
| `PAYMENTS_READ` | reading a Terminal checkout's status and the payment it made — the poll and every webhook re-fetch |
| `DEVICE_CREDENTIAL_MANAGEMENT` | pairing a Terminal with a device code |
| `CUSTOMERS_READ`, `CUSTOMERS_WRITE` | contact sync to Square Customers (drop both if that sync is turned off) |

Not needed: `ITEMS_READ` / `INVENTORY_READ` (prices come from this app's own
catalogue), `MERCHANT_PROFILE_READ`. Never to be granted: `BANK_ACCOUNTS_READ`,
`BANK_ACCOUNTS_WRITE`, `PAYOUTS_READ`, `CASH_DRAWER_READ`, or anything else.

### Connect Square (OAuth)

1. In the Square Developer Dashboard, on the application's **OAuth** page for the
   environment in use, set the **Redirect URL** to
   `https://app.theblendbarokc.com/api/square/oauth/callback` (exactly).
2. Put that environment's **Application ID** and **Application Secret** in `.env`
   as `SQUARE_OAUTH_APPLICATION_ID` / `SQUARE_OAUTH_APPLICATION_SECRET`, with
   `SQUARE_ENV` and `SQUARE_LOCATION_ID` for the same environment, then
   `docker compose up -d api`. Credentials for the *other* environment are
   ignored (production ids start `sq0idp-`, sandbox `sandbox-sq0idb-`).
3. Admin → Billing → **Connect Square**. Sign in to Square, check the permission
   list, Allow. The Billing tab shows the merchant and the permissions Square
   actually granted, and flags any missing or extra.

Once OAuth is configured for the environment, **`SQUARE_ACCESS_TOKEN` is
ignored** — remove it. Until Connect Square is done the client has no token:
payments fail with "not connected", the public checkout guard stays shut, and
contact sync waits rather than burning its retries.

The token is a 0600 file on the secrets volume
(`/var/lib/blendbar/secrets/square-oauth-<env>.json`), never a database row, so
it is not in backups. It lasts 30 days and is renewed about weekly by a worker
(and by **Renew now**); a failed renewal shows on the Billing tab with three
weeks to spare. **Disconnect** revokes it at Square. Connecting is recorded in
the audit log (path only — the authorization code is not stored).

Routes (admin): `POST /api/square/oauth/start` → `{url}`, `GET
/api/square/oauth/callback`, `POST /api/square/oauth/refresh`, `POST
/api/square/oauth/disconnect`. Status in `GET /api/square/status` → `connection`.

The reconciliation report was removed on 2026-10-08 at the owner's request; the
`reconciliation_runs` table is kept as read-only history.

### Inbound webhooks

`POST /api/webhooks/square` is public (Square can't present an employee session) but
**signature-verified**, which is the entire security boundary — it can mark carts
paid. Square signs `notification_url + raw_body` with HMAC-SHA256, base64-encoded, in
`x-square-hmacsha256-signature`.

`SQUARE_WEBHOOK_URL` is configured explicitly rather than reconstructed from request
headers: `Host` is attacker-influenced, and deriving the signed message from it would
let a caller choose the string their forged signature was computed over. It must match
what's in the Square dashboard **byte for byte**, trailing slash included.

**With `SQUARE_WEBHOOK_SIGNATURE_KEY` or `SQUARE_WEBHOOK_URL` unset the receiver is
disabled and returns 503** — it never runs unauthenticated. Until webhooks are wired,
"Check Square" on the checkout screen (`POST /api/carts/:id/refresh`) pulls the payment
state directly; that button is also the recovery path for a webhook lost to a deploy.

Events acted on: `payment.*` and `refund.*` (settle the cart),
`terminal.checkout.*` (re-fetch the checkout and settle), and `device.code.*`
(finish pairing). Everything else is filed and ignored.

- `GET /api/square/status` — backend, live-vs-mock, webhook state, cart counts, Terminals.
- `GET /api/square/events` — recent inbound webhooks, for debugging.

## Email (Google Workspace)

Two messages leave the app:

| Message | When | Optional? |
|---|---|---|
| **Sign-in link** | A customer asks to enter the portal | **No** — it is the only way in |
| **Your blend is ready** | Staff press "Mark ready" on a paid order | Yes, toggle in Admin → Email |

Configured in **Admin → Email**: sender address, sender name, reply-to, the
order-ready toggle, a test-send button, and a log of what was sent. Relay host and
credentials are **not** settable there — they live in the server environment, like
every other secret in this app.

### Two transports, picked in order

**1. Gmail API with a service account (preferred).** Set `GOOGLE_SA_KEY_FILE` and
`GOOGLE_IMPERSONATE`.

**2. Workspace SMTP relay.** Set `SMTP_HOST`.

**3. Otherwise a mock** that logs instead of sending.

Both are OAuth-era mechanisms; the older options are closed off. Basic
username/password SMTP is being disabled. App passwords still work but Google's
own docs call them "not recommended" — they cannot be scoped, and the 16
characters grant full send rights on the mailbox.

The Gmail API is preferred because it does not depend on the server's IP address,
it sends as a real mailbox (so messages appear in that account's **Sent** folder),
and nothing expires. A *service account* is used rather than three-legged user
consent deliberately: a user refresh token can be revoked or lapse, and when it
does, mail stops until a human signs in again — for sign-in links, which are the
only way into the customer portal, that failure arrives at 2am and locks customers
out. A service account has no such moment.

> **Note on Google verification:** the only scope requested is `gmail.send`, which
> is *sensitive*, not *restricted* — it cannot read a single message. And for an
> app used only inside its own Workspace, Google requires **no verification and no
> CASA assessment** at all. Earlier revisions of this README overstated that
> barrier.

#### Setting up the Gmail API (preferred)

1. Google Cloud console → new project → **enable the Gmail API** → create a
   **service account** → Keys → Add key → **JSON**.
2. Google Admin → **Security → Access and data control → API controls →
   Domain-wide delegation → Add new**:
   - Client ID: the service account's numeric client ID
   - Scopes: `https://www.googleapis.com/auth/gmail.send`
3. **Admin → Email → Google connection**: paste the JSON key, enter the mailbox to
   send as, press **Connect Google**. No restart and no shell access needed — the
   mailer is swapped in place.
4. Set a **From address** and press **Send test**.

The uploaded key is written to a file on a mounted volume (`email_secrets`), not
to a database column. That is deliberate: `GET /api/admin/backup` hands an admin a
full `pg_dump`, and a credential in a table would ride along in every copy of that
file. It is never read back out to a browser — the panel only ever shows the
service-account address.

For an ops-managed deployment, `GOOGLE_SA_KEY_HOST_PATH` + `GOOGLE_IMPERSONATE` in
`.env` still work and **take precedence**; the admin form then shows as read-only,
so there is only ever one source of truth.

The From address should be the impersonated mailbox, or one of its verified
"send mail as" aliases — Gmail rejects or rewrites anything else.

A file path is preferred over `GOOGLE_SA_KEY_JSON` because it keeps the private
key out of the process environment, where it would show up in `docker inspect`.

#### Setting up the SMTP relay (alternative)

1. Google Admin → **Apps → Google Workspace → Gmail → Routing → SMTP relay service**.
2. Allowed senders: **Only addresses in my domains**.
3. Authentication: tick **Only accept mail from the specified IP addresses** and
   add this server's public IP.
4. Encryption: tick **Require TLS encryption**.
5. Set `SMTP_HOST=smtp-relay.gmail.com` and `SMTP_PORT=587`, restart, then set a
   From address and send a test.

With IP allowlisting there are no credentials to store at all — the trade is that
it breaks if the server's address changes, and relay mail does not appear in a
Sent folder. If you also enable "Require SMTP Authentication", set
`SMTP_USERNAME` and `SMTP_PASSWORD` together; the app refuses a half-configured
login rather than silently falling back to an unauthenticated connection.

Either way the From address must be a mailbox on the Workspace domain, and
SPF/DKIM/DMARC should be in place for the domain or messages will land in spam.

### Design notes

- **No message body is ever stored.** `email_deliveries` records metadata only. A
  sign-in email contains a token that grants a customer session, and that token is
  already held hashed in `customer_login_tokens` — persisting the rendered body
  would leave a working credential at rest in a second place. Queued mail is
  re-rendered from ids at send time.
- **Sign-in links send inline; everything else queues.** Someone is watching a
  "check your email" screen and the token expires in minutes, so that path does
  not wait for a worker tick. Order-ready mail goes through the queue with
  exponential backoff.
- **The request-link endpoint answers identically whether or not the address
  exists**, including when sending fails, so it cannot be used to discover who is
  a customer.
- **TLS is required, not preferred.** The transport refuses to fall back to an
  unencrypted connection, which would put sign-in links on the wire in clear.
- Every message is **plain text and HTML**. A sign-in link that only renders in an
  HTML client fails for anyone whose mail app blocks HTML.
- One "your blend is ready" per order, enforced by a partial unique index —
  pressing the button twice does not email the customer twice.

> **Until `SMTP_HOST` is set** the app uses a mock that writes messages to the
> server log. Sign-in links therefore never arrive, and the customer portal is
> effectively closed — which is why `PORTAL_BYPASS_EMAIL` still exists. Remove
> that bypass once the relay is live.

## Chat notifications (Discord / Slack / Teams)

Posts to a chat channel when **a customer** does something. Two events, and only
two:

| Event | Fires when | Why it earns an interruption |
|---|---|---|
| `sale.online` | A cart with no employee behind it is paid — i.e. bought from a `/s/<id>` share link | Nobody was at the till, and the blend does not exist yet |
| `event.booked` | A cart containing an `event_deposit` line is paid | The published booking terms say *"Without a deposit, your event is not booked"* — that payment **is** the booking |

**A sale rung up at the bar does not notify.** The staff member who took it is
standing right there; announcing it is noise, and a channel full of noise stops
being read. The discriminator is `carts.created_by`: null means the public
checkout built it, set means an employee did.

An event deposit notifies either way, because staff raise the invoice but the
customer decides when to pay it — and that moment is what the calendar needs.

Managed in **Admin → Chat notifications**: add a channel, send a test message,
pause it, or remove it. Per channel you choose which events it receives and
whether the customer's email is included.

### How it is kept safe

- **The webhook URL is a bearer credential** — anyone holding it can post to the
  channel. It is stored server-side and never returned to the browser; the API
  gives back a redacted hint (`discord.com/…abcd`) instead.
- **Host allowlist per platform**, not a private-IP blocklist. This endpoint takes
  a URL from an admin and makes the server fetch it, which is textbook SSRF. A
  blocklist must anticipate every spelling of an internal address; an allowlist
  only has to name the hosts that are ever correct. Non-https, credentials in the
  URL (`https://hooks.slack.com@evil.example/…`), and lookalike domains
  (`discord.com.evil.example`) are all rejected.
- **Customer email is off by default.** The order details are enough to act on,
  and a chat channel is a third party — sending PII there should be a decision
  someone makes, not something that happens quietly.
- **Delivery runs on the background worker**, never on the payment path: a Discord
  outage must not be able to fail a customer's checkout.
- **Deduped on `(target, cart, event)`.** Payment settlement is idempotent and may
  run again (webhook redelivery, staff pressing "Check Square"), so without this
  one payment could ping the channel repeatedly.

`cart_items.kind` (`blend` / `event_deposit` / `fee` / `other`) is what makes the
deposit trigger reliable. It is set explicitly by the operator — matching a
deposit on its free-text label would break the first time someone retyped it.

> **Teams note:** the payload is an Office 365 connector *MessageCard*, which is
> what a Teams **Incoming Webhook** accepts. Microsoft is retiring those in favour
> of Workflows (Power Automate), which take Adaptive Cards instead. Existing
> connector URLs still work; a channel migrated to a Workflow URL will need a new
> payload shape.

### Contact sync

Customers still flow outward to **Square Customers** through the transactional
`sync_outbox` and its background worker (exponential backoff, at-least-once). Marketing
consent maps to Square's inverse field, `preferences.email_unsubscribed`, so someone
who did not opt in is not mailed. Orders no longer go through the outbox — they reach
Square through checkout, which has to be synchronous because an operator is standing
there waiting for a link.

- `GET /api/sync/status` — contact job counts, recent failures.
- `POST /api/sync/retry` — requeue every failed job.

### Going live on Square

The app runs on an **in-process mock** until credentials are set: it produces fake
payment links, charges nobody, and says so in red on both the checkout screen and the
admin panel. The mock still exercises the full path (checkout or Terminal → payment →
settlement), which is how the logic is tested without credentials.

The live HTTP client in `api/src/square/http.rs` has been exercised against **Square
Sandbox** (2026-10-01, steps 1–3 below) but has **not yet taken a production
payment**. Work through this before taking real money — ✅ marks what is done:

1. ✅ In the Square developer dashboard, create an application. Take the **Sandbox**
   access token and a sandbox **location id** (`GET /v2/locations`). The
   application id is **not** needed — hosted checkout uses the server-side token only.
2. ✅ Set `SQUARE_ENV=sandbox`, `SQUARE_ACCESS_TOKEN`, `SQUARE_LOCATION_ID` in `.env`,
   then `docker compose up -d --build api`. The startup log should read
   `square: live HTTP client env="sandbox"` and Admin → Square billing
   "Live — square-sandbox".
3. ✅ Take a full test payment with a [Square test card][sq-test] end to end: build a
   cart, scan the QR, pay, and confirm the cart flips to paid. *Done 2026-10-01: a
   $1.00 cart matched Square on order, line, amount, payment id and location. It was
   settled with "Check Square" because webhooks were not yet on — re-test after step 4
   to see it flip **on its own**.*
   ✅ *Terminal, 2026-10-08: against Square Sandbox's test devices — approved, buyer
   canceled then re-sent and paid, offline then canceled, and canceled mid-sale —
   plus a signed `terminal.checkout.updated` webhook settling a cart on its own.*
4. Create the webhook subscription pointing at
   `https://<your-domain>/api/webhooks/square`. Subscribe to `payment.created`,
   `payment.updated`, `refund.created`, `refund.updated`,
   `terminal.checkout.created`, `terminal.checkout.updated` and
   `device.code.paired` — anything else (e.g. `order.*`) is filed and ignored. Put
   Square's signature key in `SQUARE_WEBHOOK_SIGNATURE_KEY` and the identical URL in
   `SQUARE_WEBHOOK_URL`. Restart, then confirm events land in Admin → Recent Square
   events.
5. Buy a **Square Terminal**, then switch to production: production token (or OAuth,
   with the permissions above) and location, `SQUARE_ENV=production`. The API logs
   a warning at startup in that mode. Pair the Terminal from Admin → Billing.
6. Take one small real payment on the Terminal and refund it in Square; confirm the
   cart goes paid → refunded.

[sq-test]: https://developer.squareup.com/docs/devtools/sandbox/payments

## Status

Milestones 1–7 are done and validated live on the VPS: scaffold + TLS, schema,
operator auth / CRUD / intake, the operator UI, and the reorder endpoint. Billing was
migrated from Squarespace to Square (carts, hosted checkout, webhook receiver)
and, on 2026-10-08, given Square Terminal payments — validated end-to-end against the mock backend, covered by unit
tests, and as of 2026-10-01 **connected to Square Sandbox** with one staff test payment
verified against Square. Webhooks and production credentials are not yet set — see
[Going live on Square](#going-live-on-square). See `RESUME.md` for
current state and open questions.
