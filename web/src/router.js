import { createRouter, createWebHistory } from 'vue-router'

import { deviceToken } from './lib/api.js'
import IntakeView from './views/IntakeView.vue'
import LookupView from './views/LookupView.vue'
import PairDevice from './views/PairDevice.vue'

const routes = [
  { path: '/', redirect: { name: 'intake' } },
  { path: '/intake', name: 'intake', component: IntakeView },
  { path: '/lookup', name: 'lookup', component: LookupView },
  { path: '/pair', name: 'pair', component: PairDevice },
]

export const router = createRouter({
  history: createWebHistory(),
  routes,
})

// Every route but /pair needs a device token. Caddy serves index.html for all
// paths, so deep links land here and get redirected the same way.
router.beforeEach((to) => {
  if (to.name !== 'pair' && !deviceToken.value) {
    return { name: 'pair', query: { next: to.fullPath } }
  }
  if (to.name === 'pair' && deviceToken.value) {
    return { name: 'intake' }
  }
  return true
})
