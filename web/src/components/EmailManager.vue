<script setup>
/**
 * Email configuration.
 *
 * Relay host and credentials are NOT here on purpose — they live in the server's
 * environment, like the Square and chat secrets. What this panel controls is who
 * the mail appears to come from and which optional messages are on, plus a test
 * button so email can be proved before a customer depends on it.
 */
import { computed, reactive, ref, watch } from 'vue'

import { api } from '../lib/api.js'

const props = defineProps({
  state: { type: Object, default: null },
  deliveries: { type: Array, default: () => [] },
})
const emit = defineEmits(['changed'])

const form = reactive({
  from_address: '',
  from_name: '',
  reply_to: '',
  order_ready_enabled: true,
})
const testTo = ref('')
const saJson = ref('')
const impersonate = ref('')
const busy = ref('')
const error = ref('')
const notice = ref('')

watch(
  () => props.state,
  (s) => {
    if (!s?.settings) return
    form.from_address = s.settings.from_address ?? ''
    form.from_name = s.settings.from_name ?? ''
    form.reply_to = s.settings.reply_to ?? ''
    form.order_ready_enabled = s.settings.order_ready_enabled
    impersonate.value = s.google?.impersonate ?? ''
  },
  { immediate: true, deep: true },
)

const live = computed(() => Boolean(props.state?.live))
const google = computed(() => props.state?.google ?? {})
// When the key comes from the server's .env the browser must not pretend it can
// change it — two sources of truth for a credential is how they drift apart.
const envManaged = computed(() => Boolean(google.value.env_managed))

async function connectGoogle() {
  if (!saJson.value.trim() || !impersonate.value.trim()) {
    error.value = 'Paste the service account key and enter the mailbox to send as.'
    return
  }
  busy.value = 'google'
  error.value = ''
  notice.value = ''
  try {
    const res = await api.connectGoogle({
      service_account_json: saJson.value,
      impersonate: impersonate.value.trim(),
    })
    // Cleared immediately: the key is stored server-side and there is no reason
    // for it to sit in the page afterwards.
    saJson.value = ''
    notice.value = res.detail
    emit('changed')
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function disconnectGoogle() {
  if (!window.confirm('Disconnect Google? Email will stop sending until it is reconnected.')) return
  busy.value = 'google'
  error.value = ''
  notice.value = ''
  try {
    await api.disconnectGoogle()
    notice.value = 'Google disconnected.'
    emit('changed')
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}
const needsFrom = computed(() => !props.state?.settings?.from_address)

async function save() {
  busy.value = 'save'
  error.value = ''
  notice.value = ''
  try {
    await api.updateEmailSettings({
      from_address: form.from_address.trim(),
      from_name: form.from_name.trim(),
      reply_to: form.reply_to.trim(),
      order_ready_enabled: form.order_ready_enabled,
    })
    notice.value = 'Saved.'
    emit('changed')
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function sendTest() {
  busy.value = 'test'
  error.value = ''
  notice.value = ''
  try {
    const res = await api.sendTestEmail(testTo.value.trim())
    if (res.ok) notice.value = res.detail
    else error.value = res.detail
    emit('changed')
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

function formatTime(value) {
  return value ? new Date(value).toLocaleString() : '—'
}
</script>

<template>
  <div class="card">
    <h2>Email</h2>

    <p class="error" v-if="error">{{ error }}</p>
    <p class="notice" v-if="notice">{{ notice }}</p>

    <dl class="summary" v-if="props.state">
      <dt>Transport</dt>
      <dd>
        <span class="badge" :class="live ? 'ok-badge' : 'danger-badge'">
          {{ live ? `Connected — ${props.state.transport}` : 'Not configured — nothing is sent' }}
        </span>
      </dd>
      <dt>Sender</dt>
      <dd>
        <span class="badge" :class="needsFrom ? 'danger-badge' : 'ok-badge'">
          {{ needsFrom ? 'No From address set' : props.state.settings.from_address }}
        </span>
      </dd>
      <dt>Messages</dt>
      <dd>
        {{ props.state.counts.sent }} sent · {{ props.state.counts.pending }} queued ·
        <strong v-if="props.state.counts.failed" class="danger-text">
          {{ props.state.counts.failed }} failed
        </strong>
        <template v-else>0 failed</template>
      </dd>
    </dl>

    <p class="muted" v-if="live">
      Sign-in links for the customer portal are sent immediately. Everything else
      is queued and retried.
    </p>
    <p class="muted danger-text" v-else>
      Nothing is being sent. Sign-in links are only written to the server log,
      which means nobody can get into the customer portal.
    </p>

    <hr style="border: 0; border-top: 1px solid var(--border); margin: 1.2rem 0" />

    <h3 style="margin-bottom: 0.6rem">Google connection</h3>

    <dl class="summary" v-if="google.connected">
      <dt>Service account</dt>
      <dd><code>{{ google.service_account || 'set in the server environment' }}</code></dd>
      <dt>Sending as</dt>
      <dd><code>{{ google.impersonate || '—' }}</code></dd>
    </dl>

    <p class="muted" v-if="envManaged">
      The key is configured in the server’s <code>.env</code>, which takes
      precedence over anything set here. Remove <code>GOOGLE_SA_KEY_FILE</code> to
      manage it from this page instead.
    </p>

    <template v-else>
      <div class="field">
        <label>Send as (Workspace mailbox)</label>
        <input
          v-model="impersonate"
          type="email"
          autocomplete="off"
          spellcheck="false"
          placeholder="hello@theblendbarokc.com"
        />
      </div>

      <div class="field">
        <label>Service account key (JSON)</label>
        <textarea
          v-model="saJson"
          rows="5"
          spellcheck="false"
          autocomplete="off"
          :placeholder="google.connected
            ? 'Paste a new key here only if you are replacing the current one'
            : 'Paste the whole downloaded JSON key file'"
        ></textarea>
      </div>
      <p class="muted field-help">
        Google Cloud → enable the <strong>Gmail API</strong> → create a
        <strong>service account</strong> → Keys → Add key → JSON. Then in Google
        Admin → Security → API controls → <strong>Domain-wide delegation</strong>,
        authorise that service account’s client ID for the single scope
        <code>https://www.googleapis.com/auth/gmail.send</code>.
        The key is stored on the server as a file — never in the database, so it
        can’t end up inside a downloaded backup — and is never shown again here.
      </p>

      <div class="row" style="gap: 0.5rem">
        <button
          class="ghost"
          type="button"
          style="flex: none"
          :disabled="busy === 'google'"
          @click="connectGoogle"
        >
          {{ busy === 'google' ? 'Connecting…' : google.connected ? 'Replace key' : 'Connect Google' }}
        </button>
        <button
          v-if="google.connected"
          class="ghost"
          type="button"
          style="flex: none"
          :disabled="busy === 'google'"
          @click="disconnectGoogle"
        >
          Disconnect
        </button>
      </div>
    </template>

    <hr style="border: 0; border-top: 1px solid var(--border); margin: 1.2rem 0" />

    <h3 style="margin-bottom: 0.6rem">Who it comes from</h3>
    <div class="row">
      <div class="field grow">
        <label>From address</label>
        <input
          v-model="form.from_address"
          type="email"
          autocomplete="off"
          spellcheck="false"
          placeholder="hello@theblendbarokc.com"
        />
      </div>
      <div class="field grow">
        <label>Sender name</label>
        <input v-model="form.from_name" type="text" placeholder="The Blend Bar" />
      </div>
    </div>
    <!-- Below the row, not inside a column: help text in a flex-end row extends
         that column's box and pushes its neighbour out of line. -->
    <p class="muted field-help">
      The From address must be a mailbox on your Workspace domain — the relay
      refuses to send as a domain it doesn’t own.
    </p>

    <div class="field">
      <label>Reply-to (optional)</label>
      <input
        v-model="form.reply_to"
        type="email"
        autocomplete="off"
        spellcheck="false"
        placeholder="Leave blank to reply to the From address"
      />
    </div>

    <label class="check" style="margin-top: 0.6rem">
      <input type="checkbox" v-model="form.order_ready_enabled" />
      <span>Email customers when their blend is ready to collect</span>
    </label>
    <p class="muted" style="margin: 0.3rem 0 0; font-size: 0.85rem">
      Sign-in links are always sent — they’re the only way into the customer
      portal, so there’s no switch for them.
    </p>

    <button
      class="ghost"
      type="button"
      style="margin-top: 0.9rem"
      :disabled="busy === 'save'"
      @click="save"
    >
      {{ busy === 'save' ? 'Saving…' : 'Save email settings' }}
    </button>

    <hr style="border: 0; border-top: 1px solid var(--border); margin: 1.2rem 0" />

    <h3 style="margin-bottom: 0.6rem">Send a test</h3>
    <div class="row" style="align-items: flex-end; gap: 0.5rem">
      <div class="field grow">
        <label>To</label>
        <input v-model="testTo" type="email" autocomplete="off" placeholder="you@theblendbarokc.com" />
      </div>
      <button
        class="ghost"
        type="button"
        style="flex: none"
        :disabled="busy === 'test' || !testTo.trim()"
        @click="sendTest"
      >
        {{ busy === 'test' ? 'Sending…' : 'Send test' }}
      </button>
    </div>
  </div>

  <div class="card">
    <h2>Recent email</h2>
    <p class="muted" v-if="!props.deliveries.length">Nothing sent yet.</p>
    <div v-for="d in props.deliveries" :key="d.id" class="list-item" style="cursor: default">
      <span class="grow">
        <strong>{{ d.to_address }}</strong>
        <span class="muted">{{ d.subject }} · {{ formatTime(d.created_at) }}</span>
        <span class="muted danger-text" v-if="d.last_error">{{ d.last_error }}</span>
      </span>
      <span class="badge" :class="d.status === 'sent' ? 'ok-badge' : d.status === 'failed' ? 'danger-badge' : ''">
        {{ d.status }}
      </span>
    </div>
  </div>
</template>

<style scoped>
.check {
  display: flex;
  align-items: center;
  gap: 0.45rem;
}

.check input {
  width: 1.1rem;
  height: 1.1rem;
}
</style>
