/**
 * End-to-end smoke test for the operator UI, driven against a running stack.
 *
 *   TOKEN=$(docker compose exec -T api blendbar-api issue-device-token smoke | tail -1) \
 *   docker run --rm --add-host app.theblendbarokc.com:<vps-ip> \
 *     -e TOKEN -e NODE_PATH=/usr/src/app/node_modules \
 *     -v "$PWD/web/smoke.js:/usr/src/app/smoke.js:ro" -v "$PWD/out:/out" \
 *     -w /usr/src/app --entrypoint node zenika/alpine-chrome:with-puppeteer smoke.js
 *
 * The --add-host is needed because /etc/hosts on the VPS points the domain at
 * 127.0.1.1, which is not reachable from inside a container. Screenshots land
 * in /out (chmod 777 it first — the image runs as a non-root user).
 *
 * This writes real rows: one customer and one order per run.
 */
const puppeteer = require('puppeteer')

const BASE = process.env.BASE || 'https://app.theblendbarokc.com'
const TOKEN = process.env.TOKEN
const SHOTS = process.env.SHOTS !== '0'

const fail = []
const log = (...a) => console.log(...a)

;(async () => {
  if (!TOKEN) throw new Error('TOKEN env var is required')

  const browser = await puppeteer.launch({ args: ['--no-sandbox'] })
  const page = await browser.newPage()
  await page.setViewport({ width: 834, height: 1112 }) // iPad-sized

  page.on('pageerror', (e) => fail.push(`PAGE ERROR: ${e.message}`))
  page.on('console', (m) => m.type() === 'error' && fail.push(`CONSOLE ERROR: ${m.text()}`))

  // goto() can hang once assets are cached (DOMContentLoaded fires before the
  // listener attaches), so the waitForSelector after each visit is the real
  // assertion rather than the navigation promise.
  const visit = (p) =>
    page.goto(`${BASE}${p}`, { waitUntil: 'domcontentloaded', timeout: 20000 }).catch(() => {})
  const shot = (name) => (SHOTS ? page.screenshot({ path: `/out/${name}.png` }) : Promise.resolve())

  // --- Pair, through the real form ---
  await visit('/pair')
  await page.type('#token', TOKEN)
  await page.click('.primary')
  await page.waitForSelector('#email', { timeout: 15000 })
  log('PAIR: ok')

  // --- Intake: custom mix ---
  await page.type('#email', `smoke${Date.now()}@example.com`)
  await page.type('#name', 'Smoke Test')

  await page.waitForSelector('#add-ingredient')
  const options = await page.$$eval('#add-ingredient option', (os) =>
    os.map((o) => o.value).filter(Boolean),
  )
  if (!options.length) throw new Error('no active ingredients to build a mix with')
  await page.select('#add-ingredient', options[0])
  await page.waitForSelector('.mix-row')

  // Roller is a tenth of the 3.4oz base, derived at display time.
  await page.$$eval('.seg button', (bs) => bs.find((b) => b.textContent.trim() === 'Roller').click())
  await new Promise((r) => setTimeout(r, 300))
  const totals = await page.$$eval('.mix-total', (ts) => ts.map((t) => t.textContent))
  if (totals.length < 2) fail.push('derived per-size total did not render')
  log(`BUILDER: ok (${totals.length} total rows)`)
  await shot('intake')

  const valid = await page.$eval('form', (f) => f.checkValidity())
  if (!valid) fail.push('form failed native validation before submit')

  await page.click('.primary[type=submit]')
  await page.waitForSelector('.card.success', { timeout: 20000 })
  log('SUBMIT: ok')
  await shot('success')

  // --- Lookup ---
  await visit('/lookup')
  await page.waitForSelector('.list-item', { timeout: 15000 })
  const count = await page.$$eval('.list-item', (e) => e.length)
  log(`LOOKUP: ${count} customer(s)`)

  // --- Reorder, using whichever customer actually has a saved mix ---
  let reorderHref = null
  const rows = await page.$$('.list-item')
  for (let i = 0; i < rows.length && !reorderHref; i++) {
    const items = await page.$$('.list-item')
    await items[i].click()
    await page.waitForSelector('dl.summary', { timeout: 15000 })
    await new Promise((r) => setTimeout(r, 1000))
    const links = await page.$$eval('a', (as) =>
      as.filter((a) => a.textContent.trim() === 'Reorder').map((a) => a.getAttribute('href')),
    )
    if (links.length) {
      reorderHref = links[0]
      await shot('lookup')
    } else {
      await page.$$eval('.ghost', (bs) => bs.find((b) => b.textContent.includes('All customers'))?.click())
      await page.waitForSelector('.list-item', { timeout: 15000 })
    }
  }

  if (!reorderHref) {
    log('REORDER: skipped — no customer has a saved mix')
  } else {
    await visit(reorderHref)
    await page.waitForSelector('.mix-row', { timeout: 15000 })
    const email = await page.$eval('#email', (e) => e.value)
    const prefill = await page.$$eval('.mix-row', (rs) =>
      rs.map((r) => `${r.querySelector('.name').textContent.trim()}=${r.querySelector('input').value}`),
    )
    if (!email || !prefill.length) fail.push('reorder did not prefill customer/mix')
    log(`REORDER: ok — ${email} ${JSON.stringify(prefill)}`)
    await shot('reorder')
  }

  await browser.close()

  if (fail.length) {
    log('\n=== FAILURES ===')
    fail.forEach((f) => log(' - ' + f))
    process.exit(1)
  }
  log('\nALL CHECKS PASSED')
})().catch((e) => {
  console.error('FATAL:', e.message)
  fail.forEach((f) => console.log(' - ' + f))
  process.exit(1)
})
