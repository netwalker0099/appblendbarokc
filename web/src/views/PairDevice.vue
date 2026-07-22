<script setup>
import { ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import { setDeviceToken, verifyToken } from '../lib/api.js'

const route = useRoute()
const router = useRouter()

const token = ref('')
const error = ref('')
const busy = ref(false)

async function pair() {
  const value = token.value.trim()
  if (!value) return

  busy.value = true
  error.value = ''
  // Store first so the request is sent with the token, then roll back if the
  // API rejects it — a 401 clears it for us.
  setDeviceToken(value)
  try {
    await verifyToken()
    router.replace(route.query.next || { name: 'intake' })
  } catch (err) {
    error.value =
      err.status === 401 ? 'That token was not accepted. Check it and try again.' : err.message
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="card">
    <h2>Pair this device</h2>
    <p class="muted">
      Paste the operator token for this device. Issue one on the server with
      <code>docker compose exec api blendbar-api issue-device-token "&lt;label&gt;"</code>.
    </p>

    <p class="error" v-if="error">{{ error }}</p>

    <form @submit.prevent="pair">
      <div class="field">
        <label for="token">Device token</label>
        <input
          id="token"
          v-model="token"
          type="text"
          autocomplete="off"
          autocapitalize="none"
          spellcheck="false"
          placeholder="bb_…"
        />
      </div>
      <button class="primary" type="submit" :disabled="busy || !token.trim()">
        {{ busy ? 'Checking…' : 'Pair device' }}
      </button>
    </form>
  </div>
</template>
