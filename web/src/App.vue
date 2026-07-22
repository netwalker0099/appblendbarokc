<script setup>
import { useRouter } from 'vue-router'

import { clearDeviceToken, deviceToken } from './lib/api.js'

const router = useRouter()

function unpair() {
  if (!confirm('Unpair this device? You will need the token again to use it.')) return
  clearDeviceToken()
  router.push({ name: 'pair' })
}
</script>

<template>
  <header class="app-header">
    <h1>Blend Bar</h1>
    <nav class="app-nav" v-if="deviceToken">
      <RouterLink :to="{ name: 'intake' }">Intake</RouterLink>
      <RouterLink :to="{ name: 'lookup' }">Lookup</RouterLink>
      <button class="icon" type="button" title="Unpair this device" @click="unpair">Unpair</button>
    </nav>
  </header>

  <main>
    <RouterView />
  </main>
</template>
