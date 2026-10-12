<script setup>
/**
 * Connect Square (OAuth).
 *
 * The button sends the browser to Square's own page, where the owner signs in
 * and approves exactly the permissions this app uses. Square then sends the
 * browser back to /api/square/oauth/callback, which redirects here with
 * ?square=connected or ?square_error=… — read once on mount, then cleared so a
 * refresh doesn't show it again.
 *
 * The token itself never reaches the browser; this panel only sees its status.
 */
import { computed, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import { api } from '../lib/api.js'

const props = defineProps({
  /// `connection` from /api/square/status.
  connection: { type: Object, default: null },
})
const emit = defineEmits(['changed'])

const route = useRoute()
const router = useRouter()

const busy = ref('')
const error = ref('')
const notice = ref('')

const c = computed(() => props.connection)

onMounted(() => {
  const { square, square_error: squareError, ...rest } = route.query
  if (square === 'connected') notice.value = 'Square is connected.'
  if (squareError) error.value = String(squareError)
  if (square || squareError) router.replace({ name: 'admin', query: rest })
})

async function connect() {
  busy.value = 'connect'
  error.value = ''
  try {
    const { url } = await api.startSquareConnect()
    window.location.assign(url)
  } catch (err) {
    error.value = err.message
    busy.value = ''
  }
}

async function renew() {
  busy.value = 'renew'
  error.value = ''
  notice.value = ''
  try {
    await api.renewSquareToken()
    notice.value = 'Renewed. Square issued a new token.'
    emit('changed')
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

async function disconnect() {
  if (
    !confirm(
      'Disconnect Square? Payments, Terminal charges and contact sync stop until ' +
        'someone presses Connect Square again.',
    )
  )
    return
  busy.value = 'disconnect'
  error.value = ''
  notice.value = ''
  try {
    await api.disconnectSquare()
    notice.value = 'Disconnected. The token was revoked at Square.'
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
  <div class="card" v-if="c">
    <h2>Square connection</h2>

    <p class="error" v-if="error" role="alert">{{ error }}</p>
    <p class="notice" v-if="notice" role="status">{{ notice }}</p>

    <template v-if="c.oauth === 'not_configured'">
      <p class="muted">
        <template v-if="c.method === 'personal_token'">
          Connected with the personal access token in <code>.env</code>, which carries
          every permission on the Square account.
        </template>
        <template v-else>Not connected to Square.</template>
        To connect with only the permissions this app needs, add
        <code>SQUARE_OAUTH_APPLICATION_ID</code> and
        <code>SQUARE_OAUTH_APPLICATION_SECRET</code> to <code>.env</code> and restart.
      </p>
    </template>

    <template v-else-if="c.oauth === 'wrong_environment'">
      <p class="muted">
        The Square application credentials in <code>.env</code> are for
        <strong>{{ c.app_environment }}</strong>, but the server is running against the
        other environment, so Connect Square isn’t available yet.
        <template v-if="c.method === 'personal_token'">
          Payments use the personal access token in <code>.env</code> until then.
        </template>
      </p>
    </template>

    <template v-else-if="!c.connected">
      <p class="muted">
        Not connected — no payments can be taken until Square is connected. You’ll sign
        in to Square and approve these permissions, and nothing more:
      </p>
      <p class="scopes">
        <span class="badge" v-for="s in c.requested_scopes" :key="s">{{ s }}</span>
      </p>
      <p class="muted danger-text" v-if="c.last_error">{{ c.last_error }}</p>
      <div class="row">
        <button
          type="button"
          style="flex: none"
          :disabled="!c.can_connect || busy === 'connect'"
          @click="connect"
        >
          {{ busy === 'connect' ? 'Opening Square…' : 'Connect Square' }}
        </button>
      </div>
    </template>

    <template v-else>
      <dl class="summary">
        <dt>Status</dt>
        <dd><span class="badge ok-badge">Connected</span></dd>
        <dt>Square merchant</dt>
        <dd>{{ c.merchant_id || '—' }}</dd>
        <dt>Permissions</dt>
        <dd>
          <span class="scopes" v-if="c.scopes?.length">
            <span class="badge" v-for="s in c.scopes" :key="s">{{ s }}</span>
          </span>
          <span class="muted" v-else>Square didn’t report them.</span>
        </dd>
        <dt>Connected</dt>
        <dd>{{ formatTime(c.connected_at) }}</dd>
        <dt>Token renewed</dt>
        <dd>{{ formatTime(c.refreshed_at) }} · expires {{ formatTime(c.expires_at) }}</dd>
      </dl>
      <p class="muted danger-text" v-if="c.missing_scopes?.length">
        Missing {{ c.missing_scopes.join(', ') }} — something will fail. Disconnect and
        connect again, approving every permission.
      </p>
      <p class="muted danger-text" v-if="c.extra_scopes?.length">
        Also holds {{ c.extra_scopes.join(', ') }}, which this app never asked for.
      </p>
      <p class="muted danger-text" v-if="c.last_error">{{ c.last_error }}</p>
      <p class="muted field-help">
        Renews itself about once a week. If renewing keeps failing it shows here, with
        three weeks to fix it before payments stop.
      </p>
      <div class="row">
        <button class="ghost" type="button" style="flex: none" :disabled="!!busy" @click="renew">
          {{ busy === 'renew' ? 'Renewing…' : 'Renew now' }}
        </button>
        <button class="ghost" type="button" style="flex: none" :disabled="!!busy" @click="disconnect">
          {{ busy === 'disconnect' ? 'Disconnecting…' : 'Disconnect' }}
        </button>
      </div>
    </template>
  </div>
</template>

<style scoped>
.scopes {
  display: flex;
  flex-wrap: wrap;
  gap: 0.3rem;
  margin: 0.3rem 0 0.6rem;
}
</style>
