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
      <dt>Relay</dt>
      <dd>
        <span class="badge" :class="live ? 'ok-badge' : 'danger-badge'">
          {{ live ? 'Connected' : 'Not configured — nothing is sent' }}
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

    <p class="muted" v-if="!live">
      Set <code>SMTP_HOST</code> (and <code>SMTP_PORT</code>) in the server’s
      <code>.env</code> and restart. For Google Workspace use
      <code>smtp-relay.gmail.com</code> on port <code>587</code>, and allowlist this
      server’s IP address in the Google Admin console under
      <em>Apps → Google Workspace → Gmail → Routing → SMTP relay service</em>. With
      IP allowlisting no username or password is needed. These are server-side
      settings and can’t be changed from here.
    </p>
    <p class="muted" v-else>
      Sign-in links for the customer portal are sent immediately. Until a relay is
      configured they are only written to the server log, which means nobody can
      sign in to the portal.
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
        <p class="muted" style="margin: 0.3rem 0 0; font-size: 0.85rem">
          Must be a mailbox on your Workspace domain — the relay refuses to send as
          a domain it doesn’t own.
        </p>
      </div>
      <div class="field grow">
        <label>Sender name</label>
        <input v-model="form.from_name" type="text" placeholder="The Blend Bar" />
      </div>
    </div>

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
