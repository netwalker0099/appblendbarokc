import { computed, ref } from 'vue'

import { api, setOnUnauthorized } from './api.js'

/// The signed-in employee: { email, role, mfa_enrolled } or null.
export const currentUser = ref(null)
export const isAdmin = computed(() => currentUser.value?.role === 'admin')

// Any 401 (expired/invalidated session) clears auth state; the router guard then
// bounces to /login on the next navigation, and views push there on the spot.
setOnUnauthorized(() => {
  currentUser.value = null
})

/// Resolve the current session from the cookie. Safe to call repeatedly.
export async function loadUser() {
  try {
    currentUser.value = await api.me()
  } catch {
    currentUser.value = null
  }
  return currentUser.value
}

export async function logout() {
  try {
    await api.logout()
  } catch {
    // ignore; we clear locally regardless
  }
  currentUser.value = null
}
