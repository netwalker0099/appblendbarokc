/* Cookie notice — every customer page.

   A notice, not a consent prompt, because there is nothing to consent to: the
   site sets one cookie (bb_customer, the portal sign-in, only when someone signs
   in) and keeps the shop bag in localStorage. Both are strictly necessary for
   something the visitor asked for, and there is no analytics, advertising or
   third-party script. Offering "Accept / Reject" for tracking that doesn't exist
   would misdescribe the site. If tracking is ever added, this has to become a
   real choice that is made *before* anything is set.

   Shown until dismissed; the dismissal is remembered in localStorage, which the
   privacy policy lists. If storage is blocked it simply shows again next time. */
(() => {
  const KEY = 'blendbar.cookie-notice'
  const VERSION = '1' // bump if what the site stores changes, to show it again

  try {
    if (localStorage.getItem(KEY) === VERSION) return
  } catch {
    /* storage blocked: fall through and show it */
  }

  function show() {
    const box = document.createElement('section')
    box.className = 'cookie-notice'
    box.setAttribute('aria-label', 'Cookie notice')
    box.innerHTML = `
      <p>
        We use only the cookies and storage this site needs to work — to keep you
        signed in and remember your bag. No tracking or ads.
        <a href="/privacy#cookies">Learn more</a>
      </p>
      <button type="button" class="btn solid">Got it</button>`
    box.querySelector('button').addEventListener('click', () => {
      try {
        localStorage.setItem(KEY, VERSION)
      } catch {
        /* nothing to remember it in; it just goes for now */
      }
      box.remove()
    })
    document.body.appendChild(box)
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', show)
  else show()
})()
