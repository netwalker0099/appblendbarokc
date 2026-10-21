<script setup>
import { onMounted } from 'vue'
import { useRouter } from 'vue-router'

import { currentUser, isAdmin, logout } from './lib/auth.js'
import { updateAvailable, watchForUpdates } from './lib/version.js'

const router = useRouter()

onMounted(watchForUpdates)

function reload() {
  window.location.reload()
}

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

  <div class="update-banner" v-if="updateAvailable" role="status">
    <span>This app has been updated. Reload to use the latest version — finish or
    save anything you’re in the middle of first.</span>
    <button type="button" @click="reload">Reload</button>
  </div>

  <main>
    <RouterView />
  </main>
</template>

<style scoped>
.update-banner {
  display: flex;
  align-items: center;
  gap: 0.8rem;
  flex-wrap: wrap;
  padding: 0.7rem 1rem;
  background: var(--ink, #211c15);
  color: #fff;
  font-size: 0.92rem;
}
.update-banner span {
  flex: 1;
  min-width: 14rem;
}
.update-banner button {
  flex: none;
}
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
