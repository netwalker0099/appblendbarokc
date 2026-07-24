<script setup>
import { ref } from 'vue'

import { api } from '../lib/api.js'
import { currentUser } from '../lib/auth.js'

const current = ref('')
const next = ref('')
const confirm = ref('')

const busy = ref(false)
const error = ref('')
const done = ref(false)

async function submit() {
  error.value = ''
  done.value = false
  if (next.value.length < 8) {
    error.value = 'New password must be at least 8 characters.'
    return
  }
  if (next.value !== confirm.value) {
    error.value = 'New passwords don’t match.'
    return
  }
  busy.value = true
  try {
    await api.changePassword(current.value, next.value)
    done.value = true
    current.value = next.value = confirm.value = ''
  } catch (err) {
    error.value = err.status === 401 ? 'Current password is incorrect.' : err.message
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="account">
    <div class="card">
      <h2>My account</h2>
      <dl class="summary" v-if="currentUser">
        <dt>Email</dt>
        <dd>{{ currentUser.email }}</dd>
        <dt>Role</dt>
        <dd>{{ currentUser.role }}</dd>
      </dl>
    </div>

    <div class="card">
      <h2>Change password</h2>
      <p class="notice" v-if="done">Password updated. Other sessions have been signed out.</p>
      <p class="error" v-if="error">{{ error }}</p>
      <form @submit.prevent="submit">
        <div class="field">
          <label for="cur">Current password</label>
          <input id="cur" v-model="current" type="password" autocomplete="current-password" required />
        </div>
        <div class="field">
          <label for="new">New password</label>
          <input id="new" v-model="next" type="password" autocomplete="new-password" minlength="8" required />
        </div>
        <div class="field">
          <label for="conf">Confirm new password</label>
          <input id="conf" v-model="confirm" type="password" autocomplete="new-password" required />
        </div>
        <button class="primary" type="submit" :disabled="busy">
          {{ busy ? 'Updating…' : 'Update password' }}
        </button>
      </form>
    </div>
  </div>
</template>

<style scoped>
.account {
  max-width: 520px;
  margin: 0 auto;
}
</style>
