import { createRouter, createWebHistory } from 'vue-router'

import { currentUser, loadUser } from './lib/auth.js'
import AccountView from './views/AccountView.vue'
import AdminView from './views/AdminView.vue'
import IntakeView from './views/IntakeView.vue'
import LoginView from './views/LoginView.vue'
import LookupView from './views/LookupView.vue'

const routes = [
  { path: '/', redirect: { name: 'intake' } },
  { path: '/intake', name: 'intake', component: IntakeView },
  { path: '/lookup', name: 'lookup', component: LookupView },
  { path: '/admin', name: 'admin', component: AdminView, meta: { admin: true } },
  { path: '/account', name: 'account', component: AccountView },
  { path: '/login', name: 'login', component: LoginView },
]

export const router = createRouter({
  history: createWebHistory(),
  routes,
})

// Resolve the session once (from the httpOnly cookie), then gate by auth + role.
let authChecked = false
router.beforeEach(async (to) => {
  if (!authChecked) {
    await loadUser()
    authChecked = true
  }

  if (to.name === 'login') {
    return currentUser.value ? { name: 'intake' } : true
  }
  if (!currentUser.value) {
    return { name: 'login', query: to.fullPath === '/' ? {} : { next: to.fullPath } }
  }
  // Workers can't reach admin-only routes.
  if (to.meta.admin && currentUser.value.role !== 'admin') {
    return { name: 'intake' }
  }
  return true
})
