import { ref } from 'vue'

const TOKEN_KEY = 'blendbar.device_token'

/// The operator device token from `issue-device-token`. It is a device
/// credential by design, so the stand tablet keeps it in localStorage and
/// re-pairs if it is ever revoked.
export const deviceToken = ref(localStorage.getItem(TOKEN_KEY) || '')

export function setDeviceToken(token) {
  deviceToken.value = token
  localStorage.setItem(TOKEN_KEY, token)
}

export function clearDeviceToken() {
  deviceToken.value = ''
  localStorage.removeItem(TOKEN_KEY)
}

export class ApiError extends Error {
  constructor(message, status) {
    super(message)
    this.name = 'ApiError'
    this.status = status
  }
}

async function request(path, { method = 'GET', body, headers = {} } = {}) {
  const res = await fetch(`/api${path}`, {
    method,
    headers: {
      ...(body ? { 'Content-Type': 'application/json' } : {}),
      ...(deviceToken.value ? { Authorization: `Bearer ${deviceToken.value}` } : {}),
      ...headers,
    },
    body: body ? JSON.stringify(body) : undefined,
  })

  let payload = null
  try {
    payload = await res.json()
  } catch {
    // Empty or non-JSON body; leave payload null and fall through to status handling.
  }

  if (!res.ok) {
    // A revoked or mistyped token should drop the tablet back to the pairing
    // screen rather than failing every later action with "unauthorized".
    if (res.status === 401) clearDeviceToken()
    throw new ApiError(payload?.error || `request failed (${res.status})`, res.status)
  }

  return payload
}

export const api = {
  listIngredients: () => request('/ingredients'),
  listScents: () => request('/scents'),
  listCustomers: (email) =>
    request(email ? `/customers?email=${encodeURIComponent(email)}` : '/customers'),
  getCustomer: (id) => request(`/customers/${id}`),
  listCustomerMixes: (id) => request(`/customers/${id}/mixes`),
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

/// Cheapest authenticated call we have — used to prove a pasted token works
/// before we store it.
export function verifyToken() {
  return request('/ingredients')
}
