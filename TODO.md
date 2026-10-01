# TODO

Open items are `- [ ]`; tick one off with `- [x]` when done. Every open item is
shown at the start of each Claude Code session until it is ticked.

- [ ] **Fix the "Shop Now" button on the production site.** It links to
  `https://theblendbarokc.com`, which was the Squarespace store. Once the apex
  DNS points at this server, that link just reloads our own homepage and the
  store is unreachable. Decide where the shop lives (e.g. keep Squarespace on
  `shop.theblendbarokc.com`), then update the link in `site/index.html`
  (and `site-prod/index.html`), and redeploy with `docker compose up --build -d caddy`.

**Square go-live** — resume here. Full context and the verified sandbox payment
are in RESUME.md → Milestone 15 → "Next session — start here".

- [ ] **`chmod 600 /opt/app/.env`.** It is `644`; it now holds the Square token.
- [ ] **Square sandbox webhooks.** Developer dashboard → Sandbox → Webhooks →
  subscribe `https://app.theblendbarokc.com/api/webhooks/square` (exactly) to
  `payment.created`, `payment.updated`, `refund.created`, `refund.updated`. Owner
  adds the signature key to `.env` as `SQUARE_WEBHOOK_SIGNATURE_KEY`; then
  `docker compose up -d api` and check the receiver is no longer "disabled".
- [ ] **Second sandbox payment that flips to paid on its own** (no "Check Square"),
  plus a refund test and a Reconciliation run over 2026-10-01 onward.
- [ ] **Decide on the $1.00 test cart** `2ee6ac46…` on "Ryan Taylor (example)" —
  confirm that customer is a test record; cancel or keep as a marked test.
- [ ] **Commit.** ~32 files of already-deployed work plus the 2026-10-01
  public-checkout guard (`api/src/routes/public.rs`) and doc updates are
  uncommitted. Commit the guard + docs on their own.
- [ ] **Go live on Square** (after the above): production token + location,
  `SQUARE_ENV=production`, one small real payment and refund, then test a
  share-link purchase (public checkout turns on automatically in production).
