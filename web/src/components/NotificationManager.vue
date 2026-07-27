<script setup>
/**
 * Admin panel for chat notification targets.
 *
 * Deliberately narrow: only customer-triggered events reach a channel. A sale
 * rung up at the bar does not notify, because the staff member who took it is
 * standing right there and a channel full of noise stops being read.
 */
import { reactive, ref } from 'vue'

import { api } from '../lib/api.js'

const props = defineProps({
  targets: { type: Array, default: () => [] },
})
const emit = defineEmits(['changed'])

const PLATFORMS = [
  { value: 'discord', label: 'Discord', host: 'discord.com/api/webhooks/…' },
  { value: 'slack', label: 'Slack', host: 'hooks.slack.com/services/…' },
  { value: 'teams', label: 'Microsoft Teams', host: '…webhook.office.com/…' },
]

const draft = reactive({
  label: '',
  platform: 'discord',
  webhook_url: '',
  notify_online_sale: true,
  notify_event_booked: true,
  include_customer_email: false,
})

const busy = ref('')
const error = ref('')
const notice = ref('')

function hintFor(platform) {
  return PLATFORMS.find((p) => p.value === platform)?.host ?? ''
}

async function add() {
  if (!draft.label.trim() || !draft.webhook_url.trim()) {
    error.value = 'A name and a webhook URL are both required.'
    return
  }
  busy.value = 'add'
  error.value = ''
  notice.value = ''
  try {
    await api.createNotificationTarget({ ...draft })
    draft.label = ''
    draft.webhook_url = ''
    notice.value = 'Channel added. Send a test message to confirm it works.'
    emit('changed')
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function toggle(target, field) {
  busy.value = target.id
  error.value = ''
  try {
    await api.updateNotificationTarget(target.id, { [field]: !target[field] })
    emit('changed')
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function test(target) {
  busy.value = target.id
  error.value = ''
  notice.value = ''
  try {
    const res = await api.testNotificationTarget(target.id)
    if (res.ok) notice.value = `${target.label}: ${res.detail}`
    else error.value = `${target.label}: ${res.detail}`
    emit('changed')
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function remove(target) {
  busy.value = target.id
  error.value = ''
  try {
    await api.deleteNotificationTarget(target.id)
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
    <h2>Chat notifications</h2>
    <p class="muted">
      Posts to Discord, Slack or Teams when <strong>a customer</strong> does
      something — an online order from a shared scent link, or an event deposit
      being paid. Sales you ring up at the bar do not post: you already know about
      those, and a noisy channel stops being read.
    </p>

    <p class="error" v-if="error">{{ error }}</p>
    <p class="notice" v-if="notice">{{ notice }}</p>

    <div v-for="t in props.targets" :key="t.id" class="list-item" style="cursor: default">
      <span class="grow">
        <strong>{{ t.label }}</strong>
        <span class="muted">
          {{ PLATFORMS.find((p) => p.value === t.platform)?.label || t.platform }} ·
          <code>{{ t.url_hint }}</code>
        </span>
        <span class="muted">
          Online sales: <strong>{{ t.notify_online_sale ? 'on' : 'off' }}</strong> ·
          Event deposits: <strong>{{ t.notify_event_booked ? 'on' : 'off' }}</strong> ·
          Customer email: <strong>{{ t.include_customer_email ? 'included' : 'hidden' }}</strong>
        </span>
        <span class="muted" v-if="t.last_success_at">
          Last delivered {{ formatTime(t.last_success_at) }}
        </span>
        <span class="muted danger-text" v-if="t.last_error">{{ t.last_error }}</span>
      </span>
      <span class="badge" :class="t.active ? 'ok-badge' : ''">
        {{ t.active ? 'Active' : 'Paused' }}
      </span>
    </div>

    <div v-for="t in props.targets" :key="`ctl-${t.id}`" class="row target-controls">
      <span class="muted target-name">{{ t.label }}</span>
      <button class="ghost" type="button" :disabled="busy === t.id" @click="test(t)">
        Send test
      </button>
      <button class="ghost" type="button" :disabled="busy === t.id" @click="toggle(t, 'active')">
        {{ t.active ? 'Pause' : 'Resume' }}
      </button>
      <button
        class="ghost"
        type="button"
        :disabled="busy === t.id"
        @click="toggle(t, 'include_customer_email')"
      >
        {{ t.include_customer_email ? 'Hide email' : 'Include email' }}
      </button>
      <button class="ghost" type="button" :disabled="busy === t.id" @click="remove(t)">
        Remove
      </button>
    </div>

    <p class="muted" v-if="!props.targets.length">No channels yet.</p>

    <hr style="border: 0; border-top: 1px solid var(--border); margin: 1.2rem 0" />

    <h3 style="margin-bottom: 0.6rem">Add a channel</h3>
    <div class="row">
      <div>
        <label>Name</label>
        <input v-model="draft.label" type="text" placeholder="e.g. #orders" />
      </div>
      <div>
        <label>Platform</label>
        <select v-model="draft.platform">
          <option v-for="p in PLATFORMS" :key="p.value" :value="p.value">{{ p.label }}</option>
        </select>
      </div>
    </div>
    <div style="margin-top: 0.6rem">
      <label>Webhook URL</label>
      <input
        v-model="draft.webhook_url"
        type="url"
        autocomplete="off"
        spellcheck="false"
        :placeholder="`https://${hintFor(draft.platform)}`"
      />
      <p class="muted" style="margin: 0.35rem 0 0; font-size: 0.85rem">
        Create this in the chat app itself (channel settings → integrations →
        incoming webhook). Treat it like a password: anyone holding it can post to
        the channel. It is stored on the server and never shown again here.
      </p>
    </div>

    <div class="row" style="margin-top: 0.7rem; flex-wrap: wrap; gap: 1rem">
      <label class="check">
        <input type="checkbox" v-model="draft.notify_online_sale" />
        <span>Online orders</span>
      </label>
      <label class="check">
        <input type="checkbox" v-model="draft.notify_event_booked" />
        <span>Event deposits paid</span>
      </label>
      <label class="check">
        <input type="checkbox" v-model="draft.include_customer_email" />
        <span>Include customer email</span>
      </label>
    </div>
    <p class="muted" style="margin: 0.4rem 0 0; font-size: 0.85rem">
      Customer email is off by default — the order details are enough to act on, and
      a chat channel is a third party. Turn it on only if the channel is private.
    </p>

    <button
      class="ghost"
      type="button"
      style="margin-top: 0.9rem"
      :disabled="busy === 'add'"
      @click="add"
    >
      {{ busy === 'add' ? 'Adding…' : 'Add channel' }}
    </button>
  </div>
</template>

<style scoped>
.target-controls {
  gap: 0.4rem;
  flex-wrap: wrap;
  align-items: center;
  margin-bottom: 0.5rem;
}

.target-name {
  flex: none;
  min-width: 6rem;
  font-size: 0.85rem;
}

.check {
  display: flex;
  align-items: center;
  gap: 0.45rem;
  flex: none;
}

.check input {
  width: 1.1rem;
  height: 1.1rem;
}
</style>
