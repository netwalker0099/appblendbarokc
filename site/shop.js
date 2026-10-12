/* The online shop (/shop). Vanilla JS under the site's strict CSP: no inline
   script, no third-party code, every request to our own /api.

   The bag lives in this browser only (localStorage) until checkout. It holds
   ids, sizes and quantities — never prices. Every price on this page is
   re-read from the API on load, and the server prices the order again from
   scratch at checkout, so an edited bag can change what is bought but never
   what it costs. */
(function () {
  'use strict'

  const BAG_KEY = 'blendbar.bag.v1'
  const MAX_QTY = 10
  const INSTAGRAM = 'https://www.instagram.com/theblendbar.okc'

  const grid = document.getElementById('products')
  const bagBody = document.getElementById('bagBody')
  const bagLink = document.getElementById('bagLink')

  const esc = (s) =>
    String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]))
  const cents = (decimal) => Math.round(Number(decimal) * 100)
  const money = (c) => '$' + (c / 100).toFixed(2).replace(/\.00$/, '')

  let shop = null // the API's catalogue
  let bag = loadBag()
  let checkoutError = ''
  // What the buyer has typed, so re-drawing the bag (a quantity change, a
  // refreshed catalogue) never wipes their email.
  const entered = { email: '', name: '', coupon: '' }

  /* --- Bag storage ---------------------------------------------------------
     Storage can be unavailable (private mode, blocked site data). The shop
     still works then; the bag just doesn't survive a reload. */
  function loadBag() {
    try {
      const raw = JSON.parse(localStorage.getItem(BAG_KEY) || '[]')
      return Array.isArray(raw)
        ? raw.filter((l) => l && typeof l.product_id === 'string' && Number.isInteger(l.quantity))
        : []
    } catch {
      return []
    }
  }

  function saveBag() {
    try {
      localStorage.setItem(BAG_KEY, JSON.stringify(bag))
    } catch {
      /* see loadBag */
    }
  }

  const find = (id) => shop && shop.products.find((p) => p.id === id)

  /** Price of one unit of a bag line, in cents, or null if it can't be bought. */
  function unitCents(line) {
    const p = find(line.product_id)
    if (!p) return null
    if (p.kind === 'product') return cents(p.price)
    const size = p.sizes.find((s) => s.size === line.size)
    return size ? cents(size.price) : null
  }

  function lineLabel(line) {
    const p = find(line.product_id)
    if (!p) return ''
    if (p.kind === 'product') return ''
    const size = p.sizes.find((s) => s.size === line.size)
    return size ? size.label : ''
  }

  /** Drop lines for things no longer on sale, so the bag never shows a price the page can't stand behind. */
  function pruneBag() {
    const before = bag.length
    bag = bag.filter((l) => {
      const p = find(l.product_id)
      return p && !p.sold_out && unitCents(l) !== null
    })
    if (bag.length !== before) saveBag()
    return before - bag.length
  }

  function add(productId, size) {
    const existing = bag.find((l) => l.product_id === productId && (l.size || null) === (size || null))
    if (existing) existing.quantity = Math.min(MAX_QTY, existing.quantity + 1)
    else bag.push({ product_id: productId, size: size || null, quantity: 1 })
    saveBag()
    checkoutError = ''
    renderBag()
  }

  function setQty(index, qty) {
    if (qty <= 0) bag.splice(index, 1)
    else bag[index].quantity = Math.min(MAX_QTY, qty)
    saveBag()
    checkoutError = ''
    renderBag()
  }

  /* --- Routing -----------------------------------------------------------
     /shop lists everything; /shop/<id> is one item's own page (Caddy serves
     this same file for both). Plain links between them — the bag is in
     storage, so a full page load loses nothing. */
  const itemId = (location.pathname.match(/^\/shop\/([^/]+)\/?$/) || [])[1] || null
  const detail = document.getElementById('detail')
  const head = document.getElementById('shopHead')
  const itemUrl = (p) => '/shop/' + encodeURIComponent(p.id)

  function imageFor(p, lazy) {
    return p.image_url
      ? `<img src="${esc(p.image_url)}" alt="${esc(p.name)}" ${lazy ? 'loading="lazy"' : ''} />`
      : `<img class="shop-placeholder" src="/assets/img1.webp" alt="" />`
  }

  /** "$45", or "From $25" for a scent sold in more than one size. */
  function priceFrom(p) {
    if (p.kind === 'product') return money(cents(p.price))
    const low = Math.min(...p.sizes.map((s) => cents(s.price)))
    return (p.sizes.length > 1 ? 'From ' : '') + money(low)
  }

  /* --- The grid ------------------------------------------------------------
     Each card is a link to the item's page; choosing a size and adding to the
     bag happen there. */
  function card(p) {
    return `<a class="shop-card" href="${itemUrl(p)}">
      <div class="shop-media">${imageFor(p, true)}</div>
      <div class="shop-body">
        <h3>${esc(p.name)}</h3>
        <p class="shop-price">${priceFrom(p)}</p>
        ${p.sold_out ? '<p class="shop-flag">Sold out</p>' : ''}
      </div>
    </a>`
  }

  function renderProducts() {
    grid.setAttribute('aria-busy', 'false')
    if (!shop.products.length) {
      grid.innerHTML = `<div class="portal-note shop-empty">Nothing is in the shop right now. Message
        <a href="${INSTAGRAM}" rel="noopener">@theblendbar.okc</a> to order.</div>`
      return
    }
    grid.innerHTML = shop.products.map(card).join('')
  }

  /* --- One item's page ----------------------------------------------------- */
  let picked = null // the chosen size code, for a scent
  let qty = 1

  function renderDetail() {
    head.hidden = true
    grid.hidden = true
    detail.hidden = false

    const p = find(itemId)
    if (!p) {
      document.title = 'Not available — The Blend Bar'
      detail.innerHTML = `<a class="link-back" href="/shop">All products</a>
        <div class="portal-note shop-empty">That item isn&rsquo;t in the shop any more.
        <a href="/shop">See everything we have</a>.</div>`
      return
    }
    document.title = `${p.name} — The Blend Bar`

    // One size means there is nothing to choose.
    if (p.kind === 'scent' && p.sizes.length === 1) picked = p.sizes[0].size

    const sizes =
      p.kind === 'scent'
        ? `<fieldset class="sizes shop-sizes">
             <legend class="notes-label">Select your size</legend>
             ${p.sizes
               .map(
                 (s) => `<label class="size-opt">
                   <input type="radio" name="size" value="${esc(s.size)}" ${picked === s.size ? 'checked' : ''} />
                   <span>${esc(s.label)}</span><span class="size-price">${money(cents(s.price))}</span>
                 </label>`,
               )
               .join('')}
           </fieldset>`
        : ''

    const notes =
      p.notes && p.notes.length
        ? `<p class="notes-label">Notes</p>
           <div class="notes">${p.notes.map((n) => `<span class="note-chip">${esc(n)}</span>`).join('')}</div>`
        : ''

    detail.innerHTML = `<a class="link-back" href="/shop">All products</a>
      <div class="shop-detail-grid">
        <div class="shop-media shop-detail-media">${imageFor(p, false)}</div>
        <div class="shop-detail-copy">
          <h1>${esc(p.name)}</h1>
          <p class="shop-detail-price" id="detailPrice"></p>
          ${p.description ? `<p class="shop-desc">${esc(p.description)}</p>` : ''}
          ${notes}
          ${
            p.sold_out
              ? '<p class="portal-note">Sold out for now — check back soon.</p>'
              : `${sizes}
                 <div class="shop-buy">
                   <div class="bag-qty" role="group" aria-label="Quantity">
                     <button type="button" id="qtyDown" aria-label="One fewer">&minus;</button>
                     <span id="qtyValue" aria-live="polite">${qty}</span>
                     <button type="button" id="qtyUp" aria-label="One more">+</button>
                   </div>
                   <button class="btn solid" type="button" id="addBtn"></button>
                 </div>
                 <p class="shop-added" id="addedNote" role="status"></p>`
          }
        </div>
      </div>`
    updateDetail(p)
  }

  /** Price and button follow the chosen size and quantity. */
  function updateDetail(p) {
    const priceEl = document.getElementById('detailPrice')
    const addBtn = document.getElementById('addBtn')
    let unit = null
    if (p.kind === 'product') unit = cents(p.price)
    else if (picked) unit = cents(p.sizes.find((s) => s.size === picked).price)
    priceEl.textContent = unit === null ? priceFrom(p) : money(unit)
    if (!addBtn) return
    document.getElementById('qtyValue').textContent = qty
    document.getElementById('qtyDown').disabled = qty <= 1
    document.getElementById('qtyUp').disabled = qty >= MAX_QTY
    const needsSize = p.kind === 'scent' && !picked
    addBtn.disabled = needsSize
    addBtn.textContent = needsSize ? 'Select a size' : `Add to bag · ${money(unit * qty)}`
  }

  detail.addEventListener('change', (e) => {
    if (e.target.name !== 'size') return
    picked = e.target.value
    updateDetail(find(itemId))
  })

  detail.addEventListener('click', (e) => {
    const p = find(itemId)
    if (!p) return
    const id = e.target.closest('button') && e.target.closest('button').id
    if (id === 'qtyDown' || id === 'qtyUp') {
      qty = Math.min(MAX_QTY, Math.max(1, qty + (id === 'qtyUp' ? 1 : -1)))
      updateDetail(p)
    } else if (id === 'addBtn') {
      for (let i = 0; i < qty; i++) add(p.id, p.kind === 'scent' ? picked : null)
      const label = p.kind === 'scent' ? `${p.name}, ${p.sizes.find((s) => s.size === picked).label}` : p.name
      document.getElementById('addedNote').innerHTML =
        `Added ${qty > 1 ? qty + ' × ' : ''}${esc(label)} to your bag. <a href="/shop">Keep shopping</a> or <a href="#bag">check out</a>.`
      qty = 1
      updateDetail(p)
    }
  })

  /* --- Bag ---------------------------------------------------------------- */

  function totals() {
    const subtotal = bag.reduce((sum, l) => sum + (unitCents(l) || 0) * l.quantity, 0)
    const free = shop.free_shipping_over_cents
    const shipping = subtotal > 0 && !(free && subtotal >= free) ? shop.shipping_cents : 0
    return { subtotal, shipping, total: subtotal + shipping }
  }

  function renderBag() {
    const count = bag.reduce((n, l) => n + l.quantity, 0)
    bagLink.textContent = count ? `Bag (${count})` : 'Bag'

    if (!shop) return
    if (!bag.length) {
      bagBody.innerHTML = '<p class="muted">Your bag is empty.</p>'
      return
    }

    const { subtotal, shipping, total } = totals()
    const free = shop.free_shipping_over_cents
    const freeHint =
      free && shop.shipping_cents > 0 && subtotal < free
        ? `<p class="muted bag-hint">Free shipping on orders of ${money(free)} or more.</p>`
        : ''

    const lines = bag
      .map((l, i) => {
        const p = find(l.product_id)
        const unit = unitCents(l)
        return `<li class="bag-line">
          <div class="bag-line-name">
            <strong>${esc(p.name)}</strong>
            ${lineLabel(l) ? `<span class="muted">${esc(lineLabel(l))}</span>` : ''}
          </div>
          <div class="bag-qty" role="group" aria-label="Quantity of ${esc(p.name)}">
            <button type="button" data-qty="${i}" data-delta="-1" aria-label="One fewer">&minus;</button>
            <span aria-live="polite">${l.quantity}</span>
            <button type="button" data-qty="${i}" data-delta="1" aria-label="One more" ${l.quantity >= MAX_QTY ? 'disabled' : ''}>+</button>
          </div>
          <span class="bag-line-total">${money(unit * l.quantity)}</span>
        </li>`
      })
      .join('')

    const checkout = shop.checkout_open
      ? `<form id="checkoutForm" class="buy-form" novalidate>
           <label class="field">
             <span>Email</span>
             <input type="email" id="email" required autocomplete="email" inputmode="email" placeholder="you@example.com" value="${esc(entered.email)}" />
           </label>
           <label class="field">
             <span>Name <em>(optional)</em></span>
             <input type="text" id="name" autocomplete="name" placeholder="Your name" value="${esc(entered.name)}" />
           </label>
           <label class="field">
             <span>Coupon <em>(optional)</em></span>
             <input type="text" id="coupon" autocomplete="off" autocapitalize="characters" placeholder="Coupon code" value="${esc(entered.coupon)}" />
           </label>
           <p class="error-text" id="checkoutError" ${checkoutError ? '' : 'hidden'}>${esc(checkoutError)}</p>
           <button class="btn solid" type="submit" id="checkoutBtn">Checkout</button>
           <p class="muted buy-note">
             Pay securely on Square, where you&rsquo;ll also enter your shipping address.
           </p>
         </form>`
      : `<div class="portal-note">Online checkout opens soon. Message
           <a href="${INSTAGRAM}" rel="noopener">@theblendbar.okc</a> to order in the meantime.</div>`

    bagBody.innerHTML = `<ul class="bag-lines">${lines}</ul>
      <dl class="bag-totals">
        <dt>Subtotal</dt><dd>${money(subtotal)}</dd>
        <dt>Shipping</dt><dd>${shipping ? money(shipping) : 'Free'}</dd>
        <dt class="bag-total">Total</dt><dd class="bag-total">${money(total)}</dd>
      </dl>
      ${freeHint}
      ${checkout}`

    const form = document.getElementById('checkoutForm')
    if (form) form.addEventListener('submit', submit)
  }

  bagBody.addEventListener('input', (e) => {
    if (e.target.id in entered) entered[e.target.id] = e.target.value
  })

  bagBody.addEventListener('click', (e) => {
    const btn = e.target.closest('[data-qty]')
    if (!btn) return
    const i = Number(btn.getAttribute('data-qty'))
    setQty(i, bag[i].quantity + Number(btn.getAttribute('data-delta')))
  })

  /* --- Checkout ----------------------------------------------------------- */

  async function submit(e) {
    e.preventDefault()
    const btn = document.getElementById('checkoutBtn')
    const errEl = document.getElementById('checkoutError')
    const email = document.getElementById('email').value.trim()

    const fail = (msg) => {
      checkoutError = msg
      errEl.textContent = msg
      errEl.hidden = false
      btn.disabled = false
      btn.textContent = 'Checkout'
    }

    errEl.hidden = true
    if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) return fail('Please enter a valid email address.')

    btn.disabled = true
    btn.textContent = 'Taking you to checkout…'

    try {
      const res = await fetch('/api/public/shop/checkout', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          items: bag.map((l) => ({ product_id: l.product_id, size: l.size, quantity: l.quantity })),
          email,
          name: document.getElementById('name').value.trim() || null,
          coupon_code: document.getElementById('coupon').value.trim() || null,
        }),
      })
      const data = await res.json().catch(() => ({}))

      if (!res.ok) {
        // Checkout is off or Square is unreachable: a retry would fail the
        // same way, so show the way to reach the bar instead.
        if (res.status === 503) {
          shop.checkout_open = false
          renderBag()
          return
        }
        const msg = data.error ? capitalise(data.error) + '.' : 'Something went wrong. Please try again.'
        // Something may have sold out or been withdrawn since the page loaded.
        // Reload the catalogue so the page shows the truth, then say what
        // happened — after the redraw, which replaces the form.
        if (res.status === 400) {
          await refresh().catch(() => {})
          checkoutError = msg
          renderBag()
          return
        }
        return fail(msg)
      }
      // The bag is kept until the thanks page: someone who backs out of
      // Square's page should come back to it intact.
      window.location.href = data.checkout_url
    } catch {
      fail('Could not reach the checkout. Please check your connection and try again.')
    }
  }

  const capitalise = (s) => s.charAt(0).toUpperCase() + s.slice(1)

  /* --- Load --------------------------------------------------------------- */

  async function refresh() {
    const res = await fetch('/api/public/shop')
    if (!res.ok) throw new Error(String(res.status))
    shop = await res.json()
    const dropped = pruneBag()
    if (itemId) renderDetail()
    else renderProducts()
    if (dropped) checkoutError = 'Some items in your bag are no longer available and were removed.'
    renderBag()
  }

  renderBag()
  refresh().catch(() => {
    grid.setAttribute('aria-busy', 'false')
    grid.hidden = false
    detail.hidden = true
    grid.innerHTML = `<div class="portal-note bad shop-empty">The shop couldn&rsquo;t load. Please refresh, or message
      <a href="${INSTAGRAM}" rel="noopener">@theblendbar.okc</a> to order.</div>`
  })
})()
