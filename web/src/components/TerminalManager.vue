<script setup>
/**
 * Square Terminals this app can charge on.
 *
 * Pairing: this asks Square for a one-time code, which is entered on the
 * Terminal's sign-in screen ("Sign in" → "Use a device code"). The panel polls
 * until Square reports the Terminal paired. Codes last about five minutes.
 *
 * Sandbox test devices can't be paired — they don't exist physically — so they
 * are added by their published device id instead.
 */
import { computed, onMounted, onUnmounted, ref } from 'vue'

import { api } from '../lib/api.js'

const props = defineProps({
  /// From /api/square/status: whether Square is sandbox, production or mock.
  backend: { type: String, default: '' },
})

const terminals = ref([])
const busy = ref('')
const error = ref('')
const notice = ref('')
const newName = ref('Blend Bar Terminal')

const sandbox = computed(() => props.backend !== 'square-production')

/// Square's published Sandbox test devices. Payments up to $25 succeed on the
/// first two; the rest simulate what goes wrong in real life.
const TEST_DEVICES = [
  { id: '9fa747a2-25ff-48ee-b078-04381f7c828f', label: 'Approves (card, up to $25)' },
  { id: '22cd266c-6246-4c06-9983-67f0c26346b0', label: 'Approves with a 20% tip (up to $25)' },
  { id: '841100b9-ee60-4537-9bcf-e30b2ba5e215', label: 'Customer cancels' },
  { id: '0a956d49-619a-4530-8e5e-8eac603ffc5e', label: 'Times out' },
  { id: 'da40d603-c2ea-4a65-8cfd-f42e36dab0c7', label: 'Terminal offline (never picks up)' },
]
const testDevice = ref(TEST_DEVICES[0].id)

let timer = null

onMounted(load)
onUnmounted(() => clearTimeout(timer))

async function load() {
  try {
    terminals.value = await api.listTerminals()
    watchPending()
  } catch (err) {
    error.value = err.message
  }
}

/// While a code is showing, ask Square every 3 seconds whether it's been used.
function watchPending() {
  clearTimeout(timer)
  const pending = terminals.value.filter((t) => t.status === 'pending')
  if (!pending.length) return
  timer = setTimeout(async () => {
    try {
      for (const t of pending) {
        const fresh = await api.checkTerminal(t.id)
        if (fresh.status === 'paired') notice.value = `${fresh.name} is paired and ready to take payments.`
      }
      terminals.value = await api.listTerminals()
    } catch {
      // Try again on the next tick.
    }
    watchPending()
  }, 3000)
}

async function pair() {
  busy.value = 'pair'
  error.value = ''
  notice.value = ''
  try {
    await api.pairTerminal(newName.value)
    await load()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function addTest() {
  busy.value = 'test'
  error.value = ''
  notice.value = ''
  try {
    const label = TEST_DEVICES.find((d) => d.id === testDevice.value)?.label || 'Test'
    await api.addTerminalById(`Sandbox test — ${label}`, testDevice.value)
    await load()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function makeDefault(t) {
  busy.value = t.id
  error.value = ''
  try {
    await api.updateTerminal(t.id, { is_default: true })
    await load()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function remove(t) {
  const msg =
    t.status === 'paired'
      ? `Remove ${t.name}? This app will stop sending it checkouts. To fully disconnect it, also sign it out on the Terminal.`
      : `Remove ${t.name}?`
  if (!window.confirm(msg)) return
  busy.value = t.id
  error.value = ''
  try {
    await api.removeTerminal(t.id)
    await load()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

function minutesLeft(t) {
  if (!t.pair_by) return ''
  const ms = new Date(t.pair_by) - Date.now()
  return ms > 0 ? `${Math.ceil(ms / 60000)} min left` : 'expired'
}
</script>

<template>
  <div class="card">
    <h2>Square Terminals</h2>
    <p class="muted">
      Sales are sent to a Square Terminal over the internet, so staff can charge from
      a laptop or an iPad and the customer taps or inserts on the Terminal. The
      Terminal needs Wi-Fi (or ethernet) and must be signed in to this app.
    </p>

    <p class="error" v-if="error" role="alert">{{ error }}</p>
    <p class="notice" v-if="notice" role="status">{{ notice }}</p>

    <p class="muted" v-if="!terminals.length">No Terminals yet.</p>
    <div v-for="t in terminals" :key="t.id" class="list-item" style="cursor: default; flex-wrap: wrap">
      <span class="grow">
        <strong>{{ t.name }}</strong>
        <span class="muted" v-if="t.status === 'paired'">Paired · {{ t.device_id }}</span>
        <template v-else-if="t.status === 'pending'">
          <span class="pair-code" aria-label="Pairing code">{{ t.code }}</span>
          <span class="muted">
            On the Terminal: <strong>Sign in → Use a device code</strong>, then enter this
            code. Waiting… ({{ minutesLeft(t) }})
          </span>
        </template>
        <span class="muted danger-text" v-else>Code expired before it was used — remove it and pair again.</span>
      </span>
      <span class="badge ok-badge" v-if="t.is_default">Default</span>
      <button
        v-if="t.status === 'paired' && !t.is_default"
        class="ghost"
        type="button"
        style="flex: none"
        :disabled="busy === t.id"
        @click="makeDefault(t)"
      >
        Make default
      </button>
      <button class="ghost" type="button" style="flex: none" :disabled="busy === t.id" @click="remove(t)">
        Remove
      </button>
    </div>

    <h3 style="margin: 1rem 0 0.4rem">Pair a Terminal</h3>
    <div class="row">
      <div class="field grow">
        <label for="terminal-name">Name</label>
        <input id="terminal-name" v-model="newName" type="text" maxlength="60" />
      </div>
      <button
        class="ghost"
        type="button"
        style="flex: none; align-self: flex-end"
        :disabled="busy === 'pair' || !newName.trim()"
        @click="pair"
      >
        {{ busy === 'pair' ? 'Asking Square…' : 'Get a pairing code' }}
      </button>
    </div>
    <p class="muted field-help">
      Square shows a code to type on the Terminal. It works for about five minutes.
    </p>

    <template v-if="sandbox">
      <h3 style="margin: 1rem 0 0.4rem">Sandbox test devices</h3>
      <p class="muted field-help">
        Square's Sandbox can't talk to a real Terminal. These pretend devices answer
        a checkout the way a real one would, so the whole flow can be tried without
        hardware. Only offered while Square is in Sandbox.
      </p>
      <div class="row">
        <div class="field grow">
          <label for="test-device">Behaviour</label>
          <select id="test-device" v-model="testDevice">
            <option v-for="d in TEST_DEVICES" :key="d.id" :value="d.id">{{ d.label }}</option>
          </select>
        </div>
        <button
          class="ghost"
          type="button"
          style="flex: none; align-self: flex-end"
          :disabled="busy === 'test'"
          @click="addTest"
        >
          Add test device
        </button>
      </div>
    </template>
  </div>
</template>

<style scoped>
.pair-code {
  display: inline-block;
  font: 600 1.8rem/1.2 ui-monospace, "SF Mono", Menlo, monospace;
  letter-spacing: 0.25em;
  color: var(--ink);
  margin: 0.2rem 0;
}
</style>
