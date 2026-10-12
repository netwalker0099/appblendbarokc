<script setup>
/**
 * Email configuration.
 *
 * The relay itself is NOT configurable here — it lives in the server's
 * environment, like the Square and chat secrets. What this panel controls is who
 * the mail appears to come from and which optional messages are on, plus a test
 * button so email can be proved before a customer depends on it.
 *
 * It shows where mail goes and how the server authorises there, because that is
 * what someone needs when nothing is arriving. There is nothing to connect and
 * no key to paste: the Workspace relay authorises this server by its IP address.
 * The Google key that Drive backups use lives under Admin → Backups.
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
  },
  { immediate: true, deep: true },
)

const live = computed(() => Boolean(props.state?.live))
const relay = computed(() => props.state?.relay ?? null)
// The relay authorises this server by IP, so there is no credential on the box
// at all. Worth stating on the page: it is the answer to "where is the key?".
const keyless = computed(() => relay.value?.auth === 'ip-allowlist')

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

    <h3 style="margin-bottom: 0.6rem">How mail is sent</h3>

    <dl class="summary" v-if="relay">
      <dt>Relay</dt>
      <dd><code>{{ relay.host }}:{{ relay.port }}</code></dd>
      <dt>Authorised by</dt>
      <dd>
        <span class="badge" :class="keyless ? 'ok-badge' : ''">
          {{ keyless ? 'This server’s IP address' : 'SMTP login' }}
        </span>
      </dd>
    </dl>
    <p class="muted field-help" v-if="relay && keyless">
      Nothing to configure here and no key to paste: Google recognises this
      server by its IP address, so there is no credential stored on the box.
      The allowlist lives in Google Admin → Apps → Google Workspace → Gmail →
      Routing → <strong>SMTP relay service</strong>. If mail stops with a
      <code>550 5.7.0</code> about an unregistered IP, that entry is what to
      check — most often because the server’s address changed.
    </p>
    <p class="muted field-help" v-else-if="relay">
      The relay is authorised with an SMTP login held in the server’s
      environment, so it is not shown or editable here.
    </p>
    <p class="muted danger-text" v-else>
      No relay is configured, so nothing is being sent. Set <code>SMTP_HOST</code>
      in the server’s <code>.env</code> and restart the API.
    </p>

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
