<script setup>
import { ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import { api } from '../lib/api.js'
import { loadUser } from '../lib/auth.js'

const route = useRoute()
const router = useRouter()

const step = ref('login') // 'login' | 'enroll' | 'mfa'
const email = ref('')
const password = ref('')
const code = ref('')
const enroll = ref(null) // { qr, secret, otpauth_uri }

const busy = ref(false)
const error = ref('')

async function submitLogin() {
  if (busy.value) return
  busy.value = true
  error.value = ''
  try {
    const res = await api.login(email.value.trim().toLowerCase(), password.value)
    if (res.status === 'enroll_required') {
      enroll.value = await api.mfaEnroll()
      step.value = 'enroll'
    } else {
      step.value = 'mfa'
    }
    code.value = ''
  } catch (err) {
    error.value = err.status === 401 ? 'Invalid email or password.' : err.message
  } finally {
    busy.value = false
  }
}

async function submitCode() {
  if (busy.value) return
  busy.value = true
  error.value = ''
  try {
    await api.mfaVerify(code.value.trim())
    await loadUser()
    const next = typeof route.query.next === 'string' ? route.query.next : null
    router.push(next || { name: 'intake' })
  } catch (err) {
    error.value = err.status === 401 ? 'That code didn’t match. Try again.' : err.message
    code.value = ''
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="auth">
    <div class="card">
      <h1 class="auth-title">The Blend Bar</h1>
      <p class="muted auth-sub">Employee sign-in</p>

      <p class="error" v-if="error">{{ error }}</p>

      <!-- Step 1: credentials -->
      <form v-if="step === 'login'" @submit.prevent="submitLogin">
        <div class="field">
          <label for="email">Email</label>
          <input
            id="email"
            v-model="email"
            type="email"
            inputmode="email"
            autocapitalize="none"
            autocomplete="username"
            spellcheck="false"
            required
            autofocus
          />
        </div>
        <div class="field">
          <label for="password">Password</label>
          <input id="password" v-model="password" type="password" autocomplete="current-password" required />
        </div>
        <button class="primary" type="submit" :disabled="busy">
          {{ busy ? 'Signing in…' : 'Sign in' }}
        </button>
      </form>

      <!-- Step 2: first-time MFA enrollment -->
      <form v-else-if="step === 'enroll'" @submit.prevent="submitCode">
        <h2 class="enroll-h">Set up your authenticator</h2>
        <p class="muted">
          Scan this with Google Authenticator, 1Password, or any TOTP app, then enter
          the 6-digit code to finish. MFA is required for every employee.
        </p>
        <img class="qr" :src="enroll.qr" alt="Authenticator QR code" width="200" height="200" />
        <p class="muted secret">Can’t scan? Enter this key: <code>{{ enroll.secret }}</code></p>
        <div class="field">
          <label for="code">Authenticator code</label>
          <input
            id="code"
            v-model="code"
            type="text"
            inputmode="numeric"
            autocomplete="one-time-code"
            maxlength="6"
            pattern="[0-9]*"
            required
            autofocus
          />
        </div>
        <button class="primary" type="submit" :disabled="busy">
          {{ busy ? 'Verifying…' : 'Verify & finish' }}
        </button>
      </form>

      <!-- Step 3: MFA on subsequent logins -->
      <form v-else @submit.prevent="submitCode">
        <h2 class="enroll-h">Enter your code</h2>
        <p class="muted">Open your authenticator app and enter the current 6-digit code.</p>
        <div class="field">
          <label for="code2">Authenticator code</label>
          <input
            id="code2"
            v-model="code"
            type="text"
            inputmode="numeric"
            autocomplete="one-time-code"
            maxlength="6"
            pattern="[0-9]*"
            required
            autofocus
          />
        </div>
        <button class="primary" type="submit" :disabled="busy">
          {{ busy ? 'Verifying…' : 'Verify' }}
        </button>
      </form>
    </div>
  </div>
</template>

<style scoped>
.auth {
  max-width: 420px;
  margin: 2rem auto 0;
}
.auth-title {
  font-family: var(--serif);
  font-weight: 400;
  text-transform: uppercase;
  text-align: center;
  font-size: 1.4rem;
  letter-spacing: 0.2em;
  color: var(--ink);
  margin: 0 0 0.3rem;
}
.auth-sub {
  text-align: center;
  margin: 0 0 1.5rem;
}
.enroll-h {
  font-size: 1.1rem;
  margin: 0 0 0.6rem;
}
.qr {
  display: block;
  margin: 1rem auto;
  width: 200px;
  height: 200px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: #fff;
}
.secret {
  text-align: center;
  word-break: break-all;
}
</style>
