export class ApiError extends Error {
  constructor(message, status) {
    super(message)
    this.name = 'ApiError'
    this.status = status
  }
}

// Called whenever a request comes back 401, so auth state can be cleared centrally
// (set by lib/auth.js). Sessions live in an httpOnly cookie — there's no token here.
let onUnauthorized = () => {}
export function setOnUnauthorized(fn) {
  onUnauthorized = fn
}

async function request(path, { method = 'GET', body, headers = {} } = {}) {
  const res = await fetch(`/api${path}`, {
    method,
    credentials: 'same-origin', // send the session cookie
    headers: {
      ...(body ? { 'Content-Type': 'application/json' } : {}),
      ...headers,
    },
    body: body ? JSON.stringify(body) : undefined,
  })

  let payload = null
  try {
    payload = await res.json()
  } catch {
    // Empty or non-JSON body; fall through to status handling.
  }

  if (!res.ok) {
    if (res.status === 401) onUnauthorized()
    throw new ApiError(payload?.error || `request failed (${res.status})`, res.status)
  }
  return payload
}

export const api = {
  // --- auth flow ---
  login: (email, password) => request('/auth/login', { method: 'POST', body: { email, password } }),
  mfaEnroll: () => request('/auth/mfa/enroll', { method: 'POST' }),
  mfaVerify: (code) => request('/auth/mfa/verify', { method: 'POST', body: { code } }),
  logout: () => request('/auth/logout', { method: 'POST' }),
  me: () => request('/auth/me'),

  // --- catalog / operations ---
  listIngredients: () => request('/ingredients'),
  createIngredient: (name, type) => request('/ingredients', { method: 'POST', body: { name, type } }),
  updateIngredient: (id, patch) => request(`/ingredients/${id}`, { method: 'PATCH', body: patch }),
  listScents: () => request('/scents'),
  createScent: (name) => request('/scents', { method: 'POST', body: { name } }),
  updateScent: (id, patch) => request(`/scents/${id}`, { method: 'PATCH', body: patch }),
  getSyncStatus: () => request('/sync/status'),
  retrySync: () => request('/sync/retry', { method: 'POST' }),
  listWebhooks: () => request('/webhooks/recent'),
  listCustomers: (email) =>
    request(email ? `/customers?email=${encodeURIComponent(email)}` : '/customers'),
  getCustomer: (id) => request(`/customers/${id}`),
  listCustomerMixes: (id) => request(`/customers/${id}/mixes`),
  // One round trip for the lookup view: customer + mixes-with-items + orders.
  getReorder: (id) => request(`/customers/${id}/reorder`),
  getMix: (id) => request(`/mixes/${id}`),
  listOrders: (customerId) =>
    request(customerId ? `/orders?customer_id=${encodeURIComponent(customerId)}` : '/orders'),
  submitIntake: (body, idempotencyKey) =>
    request('/intake', {
      method: 'POST',
      body,
      headers: { 'Idempotency-Key': idempotencyKey },
    }),
}

/// Fetches the full DB backup as a file. Not JSON, so it bypasses `request()` —
/// returns the raw blob plus the server-provided filename for the download.
export async function downloadBackup() {
  const res = await fetch('/api/admin/backup', { credentials: 'same-origin' })
  if (res.status === 401) {
    onUnauthorized()
    throw new ApiError('unauthorized', 401)
  }
  if (!res.ok) {
    let message = `backup failed (${res.status})`
    try {
      message = (await res.json())?.error || message
    } catch {
      // non-JSON error body; keep the generic message
    }
    throw new ApiError(message, res.status)
  }
  const blob = await res.blob()
  const disposition = res.headers.get('Content-Disposition') || ''
  const match = disposition.match(/filename="?([^"]+)"?/)
  return { blob, filename: match ? match[1] : `blendbar-backup-${Date.now()}.sql` }
}
