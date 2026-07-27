/* Public scent share page. URL is /s/<scent-id> (Caddy serves this file for /s/*);
   fetches the public scent view (ingredient names + prices, never amounts). */
(function () {
  const view = document.getElementById('view')
  const esc = (s) =>
    String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]))
  const price = (p) => (p == null ? null : '$' + Number(p).toFixed(2).replace(/\.00$/, ''))
  const id = location.pathname.split('/').filter(Boolean).pop()

  function notFound() {
    view.innerHTML = `<div class="share-card">
      <img class="share-mark" src="/assets/img1.webp" alt="" />
      <p class="eyebrow">The Blend Bar</p>
      <h1>Scent unavailable</h1>
      <p class="muted">This link may be expired, or the scent is no longer available.</p>
      <a class="btn ghost" href="/">Visit The Blend Bar</a>
    </div>`
  }

  /* Shown whenever checkout can't proceed. Never leave a buyer at a dead end —
     always give them a way to reach the bar. */
  const unavailable = (msg) =>
    `<div class="portal-note">${msg} Message
     <a href="https://www.instagram.com/theblendbar.okc" rel="noopener">@theblendbar.okc</a> to order.</div>`

  function render(s) {
    const notes = (s.notes || []).map((n) => `<span class="note-chip">${esc(n)}</span>`).join('')
    /* value carries the API's size code; the label is only for the human. */
    const sizes = [
      ['3.4 oz', 'oz3_4', s.price_oz3_4],
      ['1.7 oz', 'oz1_7', s.price_oz1_7],
      ['Roller', 'roller', s.price_roller],
      ['Spray · 10 ml', 'spray', s.price_spray],
    ].filter(([, , v]) => v != null)
    const sizeRows = sizes
      .map(
        ([label, code, v], i) =>
          `<label class="size-opt"><input type="radio" name="size" value="${esc(code)}" ${i === 0 ? 'checked' : ''} /><span>${label}</span><span class="size-price">${price(v)}</span></label>`,
      )
      .join('')

    view.innerHTML = `<div class="share-card">
      <img class="share-mark" src="/assets/img1.webp" alt="" />
      <p class="eyebrow">Shared with you</p>
      <h1>${esc(s.name)}</h1>
      ${notes ? `<p class="notes-label">Notes</p><div class="notes">${notes}</div>` : ''}
      ${sizeRows ? `<div class="sizes">${sizeRows}</div>` : ''}
      ${
        sizeRows
          ? `<form id="buyForm" class="buy-form" novalidate>
               <label class="field">
                 <span>Email</span>
                 <input type="email" id="email" required autocomplete="email"
                        inputmode="email" placeholder="you@example.com" />
               </label>
               <label class="field">
                 <span>Name <em>(optional)</em></span>
                 <input type="text" id="name" autocomplete="name" placeholder="Your name" />
               </label>
               <p class="error-text" id="buyError" hidden></p>
               <button class="btn solid" type="submit" id="buyBtn" style="width:100%">
                 Buy this scent
               </button>
               <p class="muted buy-note">
                 Pay securely on Square. Blends are made by hand &mdash; we&rsquo;ll
                 email you when yours is ready.
               </p>
             </form>`
          : unavailable('This scent isn&rsquo;t available to buy online right now.')
      }
      <p class="muted share-foot">The Blend Bar &middot; Oklahoma City</p>
    </div>`

    const form = document.getElementById('buyForm')
    if (!form) return

    form.addEventListener('submit', async (e) => {
      e.preventDefault()
      const btn = document.getElementById('buyBtn')
      const errEl = document.getElementById('buyError')
      const email = document.getElementById('email').value.trim()
      const size = (document.querySelector('input[name="size"]:checked') || {}).value

      const fail = (msg) => {
        errEl.textContent = msg
        errEl.hidden = false
        btn.disabled = false
        btn.textContent = 'Buy this scent'
      }

      errEl.hidden = true
      if (!email || !email.includes('@')) return fail('Please enter a valid email address.')
      if (!size) return fail('Please choose a size.')

      btn.disabled = true
      btn.textContent = 'Taking you to checkout…'

      try {
        const res = await fetch('/api/public/checkout', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            scent_id: s.id,
            size,
            email,
            name: document.getElementById('name').value.trim() || null,
          }),
        })
        const data = await res.json().catch(() => ({}))

        if (!res.ok) {
          /* 503 means checkout isn't switched on (or Square is unreachable).
             That's a dead end for this page, so swap in the contact fallback
             rather than inviting a retry that fails identically. */
          if (res.status === 503) {
            form.outerHTML = unavailable('Online checkout isn&rsquo;t available right now.')
            return
          }
          return fail(data.error || 'Something went wrong. Please try again.')
        }
        /* Hand off to Square's hosted page — card details are entered there,
           never on this site. */
        window.location.href = data.checkout_url
      } catch {
        fail('Could not reach the checkout. Please check your connection and try again.')
      }
    })
  }

  if (!id) {
    notFound()
  } else {
    fetch('/api/public/scent/' + encodeURIComponent(id))
      .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
      .then(render)
      .catch(notFound)
  }
})()
