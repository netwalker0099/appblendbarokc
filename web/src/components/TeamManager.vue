<script setup>
import { computed, ref } from 'vue'

import { api } from '../lib/api.js'

const props = defineProps({
  employees: { type: Array, required: true },
  currentEmail: { type: String, default: '' },
})
const emit = defineEmits(['changed'])

const ROLES = [
  { value: 'worker', label: 'Worker' },
  { value: 'admin', label: 'Admin' },
]

const draftEmail = ref('')
const draftRole = ref('worker')
const busy = ref(false)
const error = ref('')
// The last generated temp password to surface once (create / reset).
const credential = ref(null) // { email, password }

const sorted = computed(() =>
  [...props.employees].sort((a, b) => Number(b.active) - Number(a.active) || a.email.localeCompare(b.email)),
)

async function run(fn) {
  busy.value = true
  error.value = ''
  try {
    await fn()
    emit('changed')
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = false
  }
}

async function addEmployee() {
  const email = draftEmail.value.trim().toLowerCase()
  if (!email || busy.value) return
  await run(async () => {
    const res = await api.createEmployee(email, draftRole.value)
    credential.value = { email: res.employee.email, password: res.temp_password }
    draftEmail.value = ''
    draftRole.value = 'worker'
  })
}

function setRole(emp, role) {
  if (role === emp.role) return
  run(() => api.updateEmployee(emp.id, { role }))
}

function toggleActive(emp) {
  run(() => api.updateEmployee(emp.id, { active: !emp.active }))
}

function resetPassword(emp) {
  if (!confirm(`Reset ${emp.email}'s password? Their current password stops working.`)) return
  run(async () => {
    const res = await api.resetEmployeePassword(emp.id)
    credential.value = { email: emp.email, password: res.temp_password }
  })
}

function resetMfa(emp) {
  if (!confirm(`Reset ${emp.email}'s MFA? They'll set up a new authenticator on next login.`)) return
  run(() => api.resetEmployeeMfa(emp.id))
}

function fmt(value) {
  return value ? new Date(value).toLocaleDateString() : '—'
}
</script>

<template>
  <div class="card">
    <h2>Team — {{ employees.filter((e) => e.active).length }} active / {{ employees.length }} total</h2>

    <p class="error" v-if="error">{{ error }}</p>

    <div class="notice" v-if="credential">
      Temporary password for <strong>{{ credential.email }}</strong>:
      <code>{{ credential.password }}</code>
      <div class="muted" style="margin-top: 0.3rem">
        Share it securely — it won’t be shown again. They’ll set up MFA and can change it under “My account”.
        <button class="ghost" type="button" style="margin-left: 0.5rem" @click="credential = null">Dismiss</button>
      </div>
    </div>

    <form class="row" @submit.prevent="addEmployee">
      <div>
        <input
          v-model="draftEmail"
          type="email"
          inputmode="email"
          autocapitalize="none"
          spellcheck="false"
          placeholder="new employee email"
          aria-label="New employee email"
        />
      </div>
      <div style="flex: none">
        <select v-model="draftRole" aria-label="New employee role">
          <option v-for="r in ROLES" :key="r.value" :value="r.value">{{ r.label }}</option>
        </select>
      </div>
      <button class="ghost" type="submit" style="flex: none" :disabled="busy || !draftEmail.trim()">Add</button>
    </form>

    <div
      v-for="emp in sorted"
      :key="emp.id"
      class="list-item team-row"
      :class="{ inactive: !emp.active }"
      style="cursor: default"
    >
      <span class="grow">
        <strong>
          {{ emp.email }}
          <span class="badge" v-if="emp.email === currentEmail">you</span>
        </strong>
        <span class="muted">
          {{ emp.mfa_enrolled ? 'MFA on' : 'MFA not set up' }} · last login {{ fmt(emp.last_login_at) }}
        </span>
      </span>
      <select
        style="flex: none; width: 8rem"
        :value="emp.role"
        :aria-label="`${emp.email} role`"
        :disabled="busy"
        @change="setRole(emp, $event.target.value)"
      >
        <option v-for="r in ROLES" :key="r.value" :value="r.value">{{ r.label }}</option>
      </select>
      <button class="ghost" type="button" style="flex: none" :disabled="busy" @click="resetPassword(emp)">
        Reset pw
      </button>
      <button class="ghost" type="button" style="flex: none" :disabled="busy" @click="resetMfa(emp)">
        Reset MFA
      </button>
      <button class="ghost" type="button" style="flex: none" :disabled="busy" @click="toggleActive(emp)">
        {{ emp.active ? 'Deactivate' : 'Activate' }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.team-row {
  flex-wrap: wrap;
}
.notice code {
  font-weight: 700;
}
</style>
