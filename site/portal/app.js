/* Customer portal app (vanilla JS; served under a strict CSP so no inline JS).
   Talks to the same-origin API proxied by the sandbox vhost. */
(function () {
  const view = document.getElementById('view')
  const esc = (s) =>
    String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]))
  const SIZES = [
    ['oz3_4', '3.4 oz'],
    ['oz1_7', '1.7 oz'],
    ['roller', 'Roller'],
    ['spray', 'Spray · 10 ml'],
  ]

  async function api(path, opts = {}) {
    const res = await fetch('/api/customer' + path, {
      method: opts.method || 'GET',
      credentials: 'same-origin',
      headers: opts.body ? { 'Content-Type': 'application/json' } : {},
      body: opts.body ? JSON.stringify(opts.body) : undefined,
    })
    if (!res.ok) throw Object.assign(new Error('request failed'), { status: res.status })
    try {
      return await res.json()
    } catch {
      return null
    }
  }

  function loginView() {
    view.innerHTML = `
      <div class="portal-card">
        <img src="/assets/img1.webp" alt="" />
        <p class="eyebrow">Customer Portal</p>
        <h1>Reorder your signature</h1>
        <p>Enter your email and we'll send you a secure sign-in link — no password to remember.</p>
        <form id="loginForm" novalidate>
          <input id="email" class="portal-input" type="email" placeholder="you@example.com" autocomplete="email" required />
          <button class="btn solid" type="submit" id="loginBtn">Send sign-in link</button>
        </form>
      </div>`
    document.getElementById('loginForm').addEventListener('submit', onLogin)
  }

  async function onLogin(e) {
    e.preventDefault()
    const email = document.getElementById('email').value.trim()
    if (!email) return
    const btn = document.getElementById('loginBtn')
    btn.disabled = true
    btn.textContent = 'Sending…'
    let res = null
    try {
      res = await api('/login', { method: 'POST', body: { email } })
    } catch {
      /* generic response regardless */
    }
    // Dev bypass: session already set — go straight to the dashboard.
    if (res && res.status === 'bypass') return init()
    sentView(email)
  }

  function sentView(email) {
    view.innerHTML = `
      <div class="portal-card">
        <img src="/assets/img1.webp" alt="" />
        <p class="eyebrow">Check your email</p>
        <h1>Link on the way</h1>
        <p>If <strong>${esc(email)}</strong> is in our system, we've sent a secure sign-in link. It expires in 15 minutes.</p>
        <p class="muted">Didn't get it? Check spam, or <a href="/portal">try again</a>.</p>
      </div>`
  }

  function blendCard(type, id, name) {
    const opts = SIZES.map(([v, l]) => `<option value="${v}">${l}</option>`).join('')
    // Only house scents have public share pages (with prices); custom blends don't.
    const shareUrl = location.origin + '/s/' + id
    const shareBtn = type === 'set_perfume' ? `<button class="btn ghost" data-share>Share</button>` : ''
    const panel =
      type === 'set_perfume'
        ? `<div class="share-panel" hidden>
        <p class="muted">Share this scent — a friend can view and buy it from this link.</p>
        <div class="share-row">
          <input class="portal-input share-link" readonly value="${esc(shareUrl)}" />
          <button class="btn ghost" data-copy>Copy</button>
        </div>
        <img class="share-qr" src="/api/public/scent/${esc(id)}/qr" alt="Scan to open this scent" width="180" height="180" />
      </div>`
        : ''
    return `<div class="blend">
      <div class="blend-name">${esc(name)}</div>
      <div class="blend-controls">
        <select class="blend-size portal-input" aria-label="Bottle size">${opts}</select>
        <button class="btn solid" data-reorder data-type="${type}" data-id="${esc(id)}">Reorder</button>
        ${shareBtn}
      </div>
      ${panel}
    </div>`
  }

  async function dashboard(me) {
    let data
    try {
      data = await api('/history')
    } catch {
      return loginView()
    }
    const mixes = data.mixes || []
    const scents = data.scents || []
    const empty = !mixes.length && !scents.length
    view.innerHTML = `
      <div class="portal-head">
        <div>
          <p class="eyebrow">Welcome back</p>
          <h1>${esc(me.name || me.email)}</h1>
        </div>
        <button class="btn ghost" id="logoutBtn">Sign out</button>
      </div>
      <div id="flash"></div>
      ${mixes.length ? `<h2 class="blend-h">Your custom blends</h2><div class="blends">${mixes.map((m) => blendCard('custom_mix', m.id, m.name || 'Custom blend')).join('')}</div>` : ''}
      ${scents.length ? `<h2 class="blend-h">Your signature scents</h2><div class="blends">${scents.map((s) => blendCard('set_perfume', s.id, s.name)).join('')}</div>` : ''}
      ${empty ? `<p class="muted">No saved blends yet — visit us at the bar to craft your first.</p>` : ''}`
    document.getElementById('logoutBtn').addEventListener('click', onLogout)
    view.querySelectorAll('[data-reorder]').forEach((b) => b.addEventListener('click', onReorder))
    view.querySelectorAll('[data-share]').forEach((b) =>
      b.addEventListener('click', (e) => {
        const panel = e.currentTarget.closest('.blend').querySelector('.share-panel')
        if (panel) panel.hidden = !panel.hidden
      }),
    )
    view.querySelectorAll('[data-copy]').forEach((b) =>
      b.addEventListener('click', async (e) => {
        const link = e.currentTarget.closest('.share-panel').querySelector('.share-link').value
        try {
          await navigator.clipboard.writeText(link)
          e.currentTarget.textContent = 'Copied!'
          setTimeout(() => (e.currentTarget.textContent = 'Copy'), 1500)
        } catch {
          /* clipboard unavailable */
        }
      }),
    )
  }

  async function onReorder(e) {
    const btn = e.currentTarget
    const size = btn.closest('.blend').querySelector('.blend-size').value
    const body = { type: btn.dataset.type, size }
    if (btn.dataset.type === 'custom_mix') body.mix_id = btn.dataset.id
    else body.scent_id = btn.dataset.id
    btn.disabled = true
    btn.textContent = 'Placing…'
    try {
      await api('/reorder', { method: 'POST', body })
      flash("Reorder placed — our team will have it ready. See you at the bar!", false)
    } catch {
      flash("Sorry, that didn't go through. Please try again.", true)
    } finally {
      btn.disabled = false
      btn.textContent = 'Reorder'
    }
  }

  function flash(msg, bad) {
    const el = document.getElementById('flash')
    if (el) el.innerHTML = `<div class="portal-note${bad ? ' bad' : ''}">${esc(msg)}</div>`
  }

  async function onLogout() {
    try {
      await api('/logout', { method: 'POST' })
    } catch {
      /* ignore */
    }
    loginView()
  }

  async function init() {
    try {
      const me = await api('/me')
      dashboard(me)
    } catch {
      loginView()
    }
  }
  init()
})()
