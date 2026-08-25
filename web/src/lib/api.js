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
  // --- package deals ---
  listBundles: () => request('/bundles'),
  createBundle: (body) => request('/bundles', { method: 'POST', body }),
  updateBundle: (id, body) => request(`/bundles/${id}`, { method: 'PATCH', body }),
  deleteBundle: (id) => request(`/bundles/${id}`, { method: 'DELETE' }),

  // --- admin deletion ---
  // POST rather than DELETE so the intent reads as deliberate in logs and
  // proxies; each refuses to remove anything money has touched.
  customerDeletionImpact: (id) => request(`/customers/${id}/deletion-impact`),
  deleteCustomer: (id) => request(`/customers/${id}/delete`, { method: 'POST' }),
  deleteMix: (id) => request(`/mixes/${id}/delete`, { method: 'POST' }),
  deleteOrder: (id) => request(`/orders/${id}/delete`, { method: 'POST' }),
  deleteIngredient: (id) => request(`/ingredients/${id}/delete`, { method: 'POST' }),
  deleteScent: (id) => request(`/scents/${id}/delete`, { method: 'POST' }),

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

  // --- email (admin) ---
  // Relay host and credentials are server-side env only; these never carry them.
  getEmailState: () => request('/email/settings'),
  updateEmailSettings: (patch) => request('/email/settings', { method: 'PATCH', body: patch }),
  sendTestEmail: (to) => request('/email/test', { method: 'POST', body: { to } }),
  listEmailDeliveries: () => request('/email/recent'),
  // The key is written to a file server-side and never returned; responses carry
  // only the service-account address.
  connectGoogle: (body) => request('/email/google', { method: 'POST', body }),
  disconnectGoogle: () => request('/email/google', { method: 'DELETE' }),

  // --- scheduled backups (admin) ---
  // The passphrase is write-only: it can be set, never read back. `status` says
  // whether one exists, not what it is.
  getBackupStatus: () => request('/admin/backup/status'),
  setBackupPassphrase: (passphrase) =>
    request('/admin/backup/passphrase', { method: 'POST', body: { passphrase } }),
  listBackupDestinations: () => request('/admin/backup/destinations'),
  createBackupDestination: (body) =>
    request('/admin/backup/destinations', { method: 'POST', body }),
  updateBackupDestination: (id, patch) =>
    request(`/admin/backup/destinations/${id}`, { method: 'PATCH', body: patch }),
  deleteBackupDestination: (id) =>
    request(`/admin/backup/destinations/${id}`, { method: 'DELETE' }),
  // Runs inline and returns the real error, which is the point of pressing it.
  runBackupNow: (id) => request(`/admin/backup/destinations/${id}/run`, { method: 'POST' }),
  listBackupRuns: () => request('/admin/backup/runs'),

  // --- audit log (admin, read-only) ---
  // There is no write/edit/delete counterpart on purpose: the log is append-only
  // and the database enforces it.
  listAuditLog: (params = {}) => {
    const q = new URLSearchParams(
      Object.entries(params).filter(([, v]) => v !== '' && v != null && v !== false),
    )
    return request(`/admin/audit${q.toString() ? `?${q}` : ''}`)
  },
  // Recomputes the hash chain server-side and reports any break.
  verifyAuditChain: () => request('/admin/audit/verify'),
  // History archived off-box and pruned. These records are permanent.
  listAuditSegments: () => request('/admin/audit/segments'),
  // Archive-then-prune, inline, so the caller sees why it refused.
  archiveAuditNow: () => request('/admin/audit/archive', { method: 'POST' }),

  // --- restore (admin) ---
  // Encrypted copies taken automatically immediately before each restore.
  listSafetyCopies: () => request('/admin/backup/safety-copies'),

  // Mark an order ready to collect; queues the customer's "it's ready" email.
  fulfilOrder: (id) => request(`/orders/${id}/fulfil`, { method: 'POST' }),
  listCustomers: (email) =>
    request(email ? `/customers?email=${encodeURIComponent(email)}` : '/customers'),
  getCustomer: (id) => request(`/customers/${id}`),
  listCustomerMixes: (id) => request(`/customers/${id}/mixes`),
  // One round trip for the lookup view: customer + mixes-with-items + orders.
  getReorder: (id) => request(`/customers/${id}/reorder`),
  getMix: (id) => request(`/mixes/${id}`),
  // Any employee may correct a saved blend — fixing a formula at the bar is
  // ordinary work, not an admin privilege.
  updateMix: (id, patch) => request(`/mixes/${id}`, { method: 'PATCH', body: patch }),
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

/// Uploads a backup file. Raw bytes, not JSON, so it bypasses `request()`.
///
/// Without `confirmPhrase` the server only inspects: it decrypts the file, loads
/// it into a scratch database and reports what is inside, leaving the live data
/// alone. Passing the phrase is what makes it destructive — opt-in via a header,
/// so a malformed request inspects rather than destroys.
export async function uploadBackupForRestore(file, confirmPhrase = null) {
  const res = await fetch('/api/admin/backup/restore', {
    method: 'POST',
    credentials: 'same-origin',
    headers: {
      'Content-Type': 'application/octet-stream',
      ...(confirmPhrase ? { 'X-Restore-Confirm': confirmPhrase } : {}),
    },
    body: file,
  })
  if (res.status === 401) {
    onUnauthorized()
    throw new ApiError('unauthorized', 401)
  }
  let payload = null
  try {
    payload = await res.json()
  } catch {
    // A restore restarts the API, so the response can be cut off mid-flight.
    // Treated as success only when the status says so.
  }
  if (!res.ok) {
    throw new ApiError(payload?.error || `restore failed (${res.status})`, res.status)
  }
  return payload
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
