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
    <h1>Blend Bar</h1>
    <nav class="app-nav" v-if="currentUser">
      <RouterLink :to="{ name: 'intake' }">Intake</RouterLink>
      <RouterLink :to="{ name: 'lookup' }">Lookup</RouterLink>
      <RouterLink v-if="isAdmin" :to="{ name: 'admin' }">Admin</RouterLink>
      <span class="who" :title="currentUser.email">{{ currentUser.email }}</span>
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
  font-size: 0.8rem;
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
