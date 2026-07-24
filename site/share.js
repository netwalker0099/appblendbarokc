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

  function render(s) {
    const notes = (s.notes || []).map((n) => `<span class="note-chip">${esc(n)}</span>`).join('')
    const sizes = [
      ['3.4 oz', s.price_oz3_4],
      ['1.7 oz', s.price_oz1_7],
      ['Roller', s.price_roller],
    ].filter(([, v]) => v != null)
    const sizeRows = sizes
      .map(
        ([label, v], i) =>
          `<label class="size-opt"><input type="radio" name="size" value="${esc(label)}" ${i === 0 ? 'checked' : ''} /><span>${label}</span><span class="size-price">${price(v)}</span></label>`,
      )
      .join('')
    view.innerHTML = `<div class="share-card">
      <img class="share-mark" src="/assets/img1.webp" alt="" />
      <p class="eyebrow">A signature scent · shared with you</p>
      <h1>${esc(s.name)}</h1>
      ${notes ? `<p class="notes-label">Notes</p><div class="notes">${notes}</div>` : ''}
      ${sizeRows ? `<div class="sizes">${sizeRows}</div>` : ''}
      <button class="btn solid" id="buyBtn" style="width:100%">Buy this scent</button>
      <p class="muted share-foot">Bespoke perfumery in Oklahoma City.</p>
    </div>`
    document.getElementById('buyBtn').addEventListener('click', () => {
      // Step 3 wires Square Hosted Checkout here.
      document.getElementById('buyBtn').outerHTML =
        `<div class="portal-note">Online checkout is launching soon — visit us at the bar or message <a href="https://www.instagram.com/theblendbar.okc" rel="noopener">@theblendbar.okc</a> to order.</div>`
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
