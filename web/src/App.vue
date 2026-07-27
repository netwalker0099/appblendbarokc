<script setup>
import { useRouter } from 'vue-router'

import { currentUser, isAdmin, logout } from './lib/auth.js'

const router = useRouter()

async function onLogout() {
  await logout()
  router.push({ name: 'login' })
}
</script>

<template>
  <header class="app-header">
    <RouterLink class="brand" :to="currentUser ? { name: 'intake' } : { name: 'login' }">
      <img class="brand-mark" src="/monogram.webp" alt="" />
      <h1>The Blend Bar</h1>
    </RouterLink>
    <nav class="app-nav" v-if="currentUser">
      <RouterLink :to="{ name: 'intake' }">Intake</RouterLink>
      <RouterLink :to="{ name: 'checkout' }">Checkout</RouterLink>
      <RouterLink :to="{ name: 'lookup' }">Lookup</RouterLink>
      <RouterLink v-if="isAdmin" :to="{ name: 'admin' }">Admin</RouterLink>
      <RouterLink class="who" :to="{ name: 'account' }" :title="`${currentUser.email} — account`">
        {{ currentUser.email }}
      </RouterLink>
      <button class="icon" type="button" @click="onLogout">Log out</button>
    </nav>
  </header>

  <main>
    <RouterView />
  </main>
</template>

<style scoped>
.who {
  color: var(--muted);
  font-size: 0.78rem;
  letter-spacing: normal;
  text-transform: none;
  max-width: 12rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
@media (max-width: 560px) {
  .who {
    display: none;
  }
}
</style>
