/* Event booking enquiry form (vanilla JS; served under a strict CSP so no
   inline JS, and no third-party captcha — script-src and connect-src are
   'self' only).

   IMPORTANT, and deliberately stated where the next person will read it: every
   check in this file is a CLIENT-SIDE check. The captcha, the honeypot and the
   timing gate all run in the visitor's browser, so anything that POSTs to the
   endpoint directly walks straight past them. They stop naive form-filling
   bots and nothing more. When the receiving endpoint is built it MUST do its
   own validation and its own rate limiting; treat this file as a courtesy to
   the human, not as a security boundary. */
(function () {
  const ENDPOINT = '/api/public/event-enquiry'
  const INSTAGRAM = 'https://www.instagram.com/theblendbar.okc'

  const form = document.getElementById('bookForm')
  const result = document.getElementById('bookResult')
  const errorEl = document.getElementById('formError')
  const submitBtn = document.getElementById('submitBtn')
  const questionEl = document.getElementById('captchaQuestion')
  const answerEl = document.getElementById('captchaAnswer')
  const newCaptchaBtn = document.getElementById('captchaNew')

  const esc = (s) =>
    String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]))

  /* --- Captcha ---------------------------------------------------------
     A worded arithmetic challenge rather than distorted characters. It is
     readable by a screen reader, needs no image, no audio alternative and no
     external service — and this form is the only booking channel on the site,
     so locking out anyone who cannot read a warped image would cost real
     bookings. */
  const WORDS = ['zero', 'one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight', 'nine', 'ten']
  let expected = null

  function newCaptcha() {
    const a = 1 + Math.floor(Math.random() * 8)
    const b = 1 + Math.floor(Math.random() * 8)
    expected = a + b
    questionEl.textContent = `What is ${WORDS[a]} plus ${WORDS[b]}?`
    answerEl.value = ''
  }

  /* Accepts the digits or the word, because someone reading "four plus three"
     may well type "seven". */
  function captchaOk() {
    const raw = answerEl.value.trim().toLowerCase()
    if (!raw) return false
    if (Number(raw) === expected) return true
    return WORDS[expected] === raw
  }

  /* --- Anti-spam signals other than the captcha ------------------------ */
  const loadedAt = Date.now()
  const MIN_FILL_MS = 3000

  function showError(msg) {
    errorEl.textContent = msg
    errorEl.hidden = false
    errorEl.scrollIntoView({ block: 'center', behavior: 'smooth' })
  }

  function clearError() {
    errorEl.hidden = true
    errorEl.textContent = ''
  }

  /* Field-by-field so the message names what to fix, rather than a blanket
     "please complete the form". */
  function firstProblem(data) {
    if (!data.first_name) return ['First name is required.', 'firstName']
    if (!data.last_name) return ['Last name is required.', 'lastName']
    if (!data.email) return ['Email is required.', 'email']
    if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(data.email)) return ['That email address does not look right.', 'email']
    if (!data.phone) return ['Phone number is required.', 'phone']
    if (data.phone.replace(/\D/g, '').length < 7) return ['That phone number looks too short.', 'phone']
    if (!data.event_date) return ['Event date is required.', 'eventDate']
    if (!data.details) return ['Event details are required.', 'details']
    return null
  }

  function collect() {
    return {
      first_name: document.getElementById('firstName').value.trim(),
      last_name: document.getElementById('lastName').value.trim(),
      email: document.getElementById('email').value.trim(),
      phone: document.getElementById('phone').value.trim(),
      event_date: document.getElementById('eventDate').value,
      details: document.getElementById('details').value.trim(),
    }
  }

  function succeeded(data) {
    form.hidden = true
    result.hidden = false
    result.innerHTML = `<div class="portal-card book-panel">
      <p class="eyebrow">Request received</p>
      <h2>Thank you, ${esc(data.first_name)}.</h2>
      <p class="muted">
        We have your details and will be in touch at ${esc(data.email)} to plan
        your event.
      </p>
      <a class="btn ghost" href="/">Back to The Blend Bar</a>
    </div>`
    result.scrollIntoView({ block: 'start', behavior: 'smooth' })
  }

  /* The send failed. Never leave someone who has just typed out their whole
     event at a dead end: show the other way to reach the bar, and show their
     own answers back so nothing they wrote is lost to a page refresh. */
  function failed(data) {
    form.hidden = true
    result.hidden = false
    result.innerHTML = `<div class="portal-card book-panel">
      <p class="eyebrow">We could not send that</p>
      <h2>Message us instead</h2>
      <p class="muted">
        Something went wrong on our side, so this request did not reach us. Your
        answers are below — send them to
        <a href="${INSTAGRAM}" rel="noopener">@theblendbar.okc</a> and we will
        pick it up from there.
      </p>
      <a class="btn solid" href="${INSTAGRAM}" rel="noopener">Message us on Instagram</a>
      <dl class="book-recap">
        <dt>Name</dt><dd>${esc(data.first_name)} ${esc(data.last_name)}</dd>
        <dt>Email</dt><dd>${esc(data.email)}</dd>
        <dt>Phone</dt><dd>${esc(data.phone)}</dd>
        <dt>Event date</dt><dd>${esc(data.event_date)}</dd>
        <dt>Details</dt><dd>${esc(data.details)}</dd>
      </dl>
      <button class="btn ghost" type="button" id="bookRetry">Try sending again</button>
    </div>`
    document.getElementById('bookRetry').addEventListener('click', () => {
      result.hidden = true
      result.innerHTML = ''
      form.hidden = false
      newCaptcha()
      clearError()
    })
    result.scrollIntoView({ block: 'start', behavior: 'smooth' })
  }

  form.addEventListener('submit', async (e) => {
    e.preventDefault()
    clearError()

    /* Honeypot and timing gate fail silently on purpose. Telling a bot which
       check caught it is free tuning information; a person will never see
       either branch. */
    if (document.getElementById('company').value) return
    if (Date.now() - loadedAt < MIN_FILL_MS) return

    const data = collect()
    const problem = firstProblem(data)
    if (problem) {
      showError(problem[0])
      document.getElementById(problem[1]).focus()
      return
    }

    if (!captchaOk()) {
      showError('That answer was not right. Here is a new question.')
      newCaptcha()
      answerEl.focus()
      return
    }

    submitBtn.disabled = true
    submitBtn.textContent = 'Sending…'
    try {
      const res = await fetch(ENDPOINT, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(data),
      })
      if (!res.ok) throw new Error('request failed with ' + res.status)
      succeeded(data)
    } catch {
      failed(data)
    } finally {
      submitBtn.disabled = false
      submitBtn.textContent = 'Submit request'
    }
  })

  newCaptchaBtn.addEventListener('click', () => {
    newCaptcha()
    answerEl.focus()
  })

  /* An event in the past is nearly always a typo or a stale datepicker. */
  const dateEl = document.getElementById('eventDate')
  dateEl.min = new Date().toISOString().slice(0, 10)

  newCaptcha()
})()
