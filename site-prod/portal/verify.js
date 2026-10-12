/* Consumes the magic-link token, then hands off to the portal app. */
(function () {
  const view = document.getElementById('view')
  const token = new URLSearchParams(location.search).get('token')

  function fail() {
    view.innerHTML = `<div class="portal-card">
      <p class="eyebrow">Customer Portal</p>
      <h1>Link expired</h1>
      <p>This sign-in link is invalid or has already been used.</p>
      <a class="btn solid" href="/portal">Request a new link</a>
    </div>`
  }

  if (!token) {
    fail()
  } else {
    fetch('/api/customer/verify', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token }),
    })
      .then((res) => {
        if (res.ok) location.replace('/portal')
        else fail()
      })
      .catch(fail)
  }
})()
