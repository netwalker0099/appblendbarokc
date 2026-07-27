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
  changePassword: (current_password, new_password) =>
    request('/auth/change-password', { method: 'POST', body: { current_password, new_password } }),

  // --- employee/user management (admin) ---
  listEmployees: () => request('/employees'),
  createEmployee: (email, role) => request('/employees', { method: 'POST', body: { email, role } }),
  updateEmployee: (id, patch) => request(`/employees/${id}`, { method: 'PATCH', body: patch }),
  resetEmployeePassword: (id) => request(`/employees/${id}/reset-password`, { method: 'POST' }),
  resetEmployeeMfa: (id) => request(`/employees/${id}/reset-mfa`, { method: 'POST' }),

  // --- catalog / operations ---
  listIngredients: () => request('/ingredients'),
  createIngredient: (name, type) => request('/ingredients', { method: 'POST', body: { name, type } }),
  updateIngredient: (id, patch) => request(`/ingredients/${id}`, { method: 'PATCH', body: patch }),
  listScents: () => request('/scents'),
  createScent: (name) => request('/scents', { method: 'POST', body: { name } }),
  updateScent: (id, patch) => request(`/scents/${id}`, { method: 'PATCH', body: patch }),
  getSettings: () => request('/settings'),
  updateSettings: (patch) => request('/settings', { method: 'PATCH', body: patch }),
  getSyncStatus: () => request('/sync/status'),
  retrySync: () => request('/sync/retry', { method: 'POST' }),

  // --- carts + Square checkout ---
  // A cart is one checkout: the unit that becomes a single Square order and a
  // single payment. Money only ever moves on Square's hosted page.
  listCarts: (customerId) =>
    request(customerId ? `/carts?customer_id=${encodeURIComponent(customerId)}` : '/carts'),
  getCart: (id) => request(`/carts/${id}`),
  createCart: (body) => request('/carts', { method: 'POST', body }),
  checkoutCart: (id) => request(`/carts/${id}/checkout`, { method: 'POST' }),
  // Pull this cart's state from Square — the backstop for a missed webhook.
  refreshCart: (id) => request(`/carts/${id}/refresh`, { method: 'POST' }),
  cancelCart: (id) => request(`/carts/${id}/cancel`, { method: 'POST' }),

  // --- Square integration (admin) ---
  getSquareStatus: () => request('/square/status'),
  reconcile: ({ from, to, save = false } = {}) => {
    const q = new URLSearchParams()
    if (from) q.set('from', from)
    if (to) q.set('to', to)
    if (save) q.set('save', 'true')
    const qs = q.toString()
    return request(`/square/reconcile${qs ? `?${qs}` : ''}`)
  },
  reconcileHistory: () => request('/square/reconcile/history'),
  listSquareEvents: () => request('/square/events'),

  // --- Chat notifications for customer-triggered events (admin) ---
  // Webhook URLs are write-only: the server returns a redacted hint, never the
  // URL itself, because holding one is enough to post into the channel.
  listNotificationTargets: () => request('/notifications/targets'),
  createNotificationTarget: (body) =>
    request('/notifications/targets', { method: 'POST', body }),
  updateNotificationTarget: (id, patch) =>
    request(`/notifications/targets/${id}`, { method: 'PATCH', body: patch }),
  deleteNotificationTarget: (id) =>
    request(`/notifications/targets/${id}`, { method: 'DELETE' }),
  testNotificationTarget: (id) =>
    request(`/notifications/targets/${id}/test`, { method: 'POST' }),
  listNotificationDeliveries: () => request('/notifications/recent'),
  listCustomers: (email) =>
    request(email ? `/customers?email=${encodeURIComponent(email)}` : '/customers'),
  getCustomer: (id) => request(`/customers/${id}`),
  listCustomerMixes: (id) => request(`/customers/${id}/mixes`),
  // One round trip for the lookup view: customer + mixes-with-items + orders.
  getReorder: (id) => request(`/customers/${id}/reorder`),
  getMix: (id) => request(`/mixes/${id}`),
  listOrders: (customerId, { uncarted = false } = {}) => {
    const q = new URLSearchParams()
    if (customerId) q.set('customer_id', customerId)
    if (uncarted) q.set('uncarted', 'true')
    const qs = q.toString()
    return request(`/orders${qs ? `?${qs}` : ''}`)
  },
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
