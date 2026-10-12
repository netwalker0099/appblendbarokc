<script setup>
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import AuditLog from '../components/AuditLog.vue'
import BackupRestore from '../components/BackupRestore.vue'
import BackupScheduler from '../components/BackupScheduler.vue'
import BundleManager from '../components/BundleManager.vue'
import CatalogManager from '../components/CatalogManager.vue'
import EmailManager from '../components/EmailManager.vue'
import EventEnquiries from '../components/EventEnquiries.vue'
import NotificationManager from '../components/NotificationManager.vue'
import ScentManager from '../components/ScentManager.vue'
import ShopManager from '../components/ShopManager.vue'
import TeamManager from '../components/TeamManager.vue'
import TerminalManager from '../components/TerminalManager.vue'
import { BOTTLE_SIZES, INGREDIENT_TYPES } from '../lib/bottle.js'
import { api, downloadBackup } from '../lib/api.js'
import { currentUser } from '../lib/auth.js'

const router = useRouter()
const route = useRoute()

/**
 * Admin had grown to ten stacked cards and a very long scroll. Grouping them
 * into tabs keeps one job on screen at a time.
 *
 * The active tab lives in the URL (`/admin?tab=billing`) rather than in
 * component state, so a refresh returns you where you were and a tab can be
 * linked to directly — which matters when telling someone "check Admin →
 * Billing".
 */
const TABS = [
  { id: 'catalog', label: 'Catalog' },
  { id: 'pricing', label: 'Pricing & packages' },
  { id: 'shop', label: 'Shop' },
  { id: 'team', label: 'Team' },
  { id: 'billing', label: 'Billing' },
  { id: 'alerts', label: 'Notifications' },
  { id: 'enquiries', label: 'Enquiries' },
  { id: 'email', label: 'Email' },
  { id: 'data', label: 'Data' },
  { id: 'activity', label: 'Activity' },
]

const activeTab = ref(TABS.some((t) => t.id === route.query.tab) ? route.query.tab : TABS[0].id)

watch(activeTab, (tab) => {
  // replace, not push: flicking through tabs shouldn't fill the back button.
  router.replace({ name: 'admin', query: { ...route.query, tab } })
})

function selectTab(id) {
  activeTab.value = id
}

/** Arrow-key movement across the tab strip, as a tablist is expected to do. */
function onTabKey(event, index) {
  const keys = { ArrowRight: 1, ArrowLeft: -1 }
  if (event.key in keys) {
    event.preventDefault()
    const next = (index + keys[event.key] + TABS.length) % TABS.length
    activeTab.value = TABS[next].id
    document.getElementById(`tab-${TABS[next].id}`)?.focus()
  } else if (event.key === 'Home' || event.key === 'End') {
    event.preventDefault()
    const target = event.key === 'Home' ? TABS[0] : TABS[TABS.length - 1]
    activeTab.value = target.id
    document.getElementById(`tab-${target.id}`)?.focus()
  }
}

/// A dot on a tab that wants attention, so a problem isn't hidden by the very
/// grouping that tidied the page up.
const tabAlert = computed(() => ({
  // Not being connected to Square means no money can be taken at all; failed
  // contact syncs rot silently. Both are fixed from the Billing tab, so the dot
  // points at the tab that holds the control.
  billing: Boolean(square.value && !square.value.live) || failedCount.value > 0,
  // No relay, or no From address, means sign-in links never arrive and the
  // customer portal is effectively closed.
  email:
    Boolean(emailState.value && !emailState.value.live) ||
    Boolean(emailState.value && !emailState.value.settings.from_address) ||
    Boolean(emailState.value && emailState.value.counts.failed > 0),
  // "No off-box backup exists" is the quietest serious problem in the whole
  // system — nothing breaks until the day everything has already been lost —
  // so it gets the same dot as a dead payment terminal.
  // Someone asked about an event and is waiting on a reply. This is the only
  // dot that means "a person is waiting", so it stays on until the list is
  // worked down rather than until something is configured.
  enquiries: openEnquiries.value > 0,
  data:
    Boolean(backupStatus.value && !backupStatus.value.passphrase_set) ||
    Boolean(backupStatus.value && !backupStatus.value.last_success_at) ||
    backupDestinations.value.some((d) => d.enabled && d.last_status === 'failed'),
}))

/** Open enquiries, reported up by the Enquiries panel to drive its tab dot. */
const openEnquiries = ref(0)

const ingredients = ref([])
const scents = ref([])
const employees = ref([])
const sync = ref(null)
const square = ref(null)
const squareEvents = ref([])
const notificationTargets = ref([])
const bundles = ref([])
const emailState = ref(null)
const emailDeliveries = ref([])
const backupStatus = ref(null)
const backupDestinations = ref([])
const backupRuns = ref([])
const referral = reactive({
  referral_enabled: false,
  referral_discount_cents: 500,
  referral_reward_cents: 500,
  coupon_expiry_days: 365,
})
const savingReferral = ref(false)

// Entered as dollars, stored as cents — the same rule as everywhere else money
// crosses a boundary in this app.
const dollars = (cents) => (Number(cents || 0) / 100).toFixed(2)
const toCents = (v) => Math.round(Number(v || 0) * 100)

async function saveReferral() {
  savingReferral.value = true
  try {
    await api.updateSettings({
      referral_enabled: referral.referral_enabled,
      referral_discount_cents: toCents(referral.referral_discount_cents),
      referral_reward_cents: toCents(referral.referral_reward_cents),
      coupon_expiry_days: Number(referral.coupon_expiry_days) || 0,
    })
    flash('Saved referral settings.')
  } catch (err) {
    handle(err)
  } finally {
    savingReferral.value = false
  }
}

/**
 * How long a device stays trusted after MFA. Kept separate from the referral
 * form so saving one can't quietly rewrite the other — this is the security
 * setting on the page, and it should only ever change when somebody pressed the
 * button under it.
 */
const security = reactive({ mfa_trust_hours: 4 })
const savingSecurity = ref(false)

async function saveSecurity() {
  savingSecurity.value = true
  try {
    const hours = Number(security.mfa_trust_hours)
    if (!Number.isInteger(hours) || hours < 0 || hours > 168) {
      throw new Error('Enter a whole number of hours between 0 and 168.')
    }
    const set = await api.updateSettings({ mfa_trust_hours: hours })
    security.mfa_trust_hours = set.mfa_trust_hours
    flash(
      hours === 0
        ? 'Saved — MFA is now required on every sign-in.'
        : `Saved — MFA won't be asked for again on a device for ${hours} hour${hours === 1 ? '' : 's'}.`,
    )
  } catch (err) {
    handle(err)
  } finally {
    savingSecurity.value = false
  }
}

// Derived from BOTTLE_SIZES rather than listed by hand, so adding a size is one
// edit in bottle.js instead of a hunt for every place that enumerates them —
// and a size can never end up sellable in intake but unpriceable here.
const CUSTOM_PRICE_KEYS = BOTTLE_SIZES.map((s) => `custom_price_${s.value}`)
const customPrices = reactive(Object.fromEntries(CUSTOM_PRICE_KEYS.map((k) => [k, ''])))
const savingPrices = ref(false)

const loading = ref(true)
const error = ref('')
const notice = ref('')

onMounted(load)

async function load() {
  loading.value = true
  error.value = ''
  try {
    const [ing, sc, emp, set, st, sq, ev, nt, bd, em, ed, bs, bdest, bruns] = await Promise.all([
      api.listIngredients(),
      api.listScents(),
      api.listEmployees(),
      api.getSettings(),
      api.getSyncStatus(),
      api.getSquareStatus(),
      api.listSquareEvents(),
      api.listNotificationTargets(),
      api.listBundles(),
      api.getEmailState(),
      api.listEmailDeliveries(),
      api.getBackupStatus(),
      api.listBackupDestinations(),
      api.listBackupRuns(),
    ])
    backupStatus.value = bs
    backupDestinations.value = bdest
    backupRuns.value = bruns
    notificationTargets.value = nt
    bundles.value = bd
    emailState.value = em
    emailDeliveries.value = ed
    ingredients.value = ing
    scents.value = sc
    employees.value = emp
    sync.value = st
    square.value = sq
    squareEvents.value = ev
    for (const k of CUSTOM_PRICE_KEYS) {
      customPrices[k] = set[k] ?? ''
    }
    referral.referral_enabled = set.referral_enabled
    referral.referral_discount_cents = dollars(set.referral_discount_cents)
    referral.referral_reward_cents = dollars(set.referral_reward_cents)
    referral.coupon_expiry_days = set.coupon_expiry_days
    security.mfa_trust_hours = set.mfa_trust_hours
  } catch (err) {
    handle(err)
  } finally {
    loading.value = false
  }
}

function handle(err) {
  error.value = err.message
  if (err.status === 401) router.push({ name: 'login' })
}

async function reloadBackups() {
  try {
    ;[backupStatus.value, backupDestinations.value, backupRuns.value] = await Promise.all([
      api.getBackupStatus(),
      api.listBackupDestinations(),
      api.listBackupRuns(),
    ])
  } catch (err) {
    handle(err)
  }
}

async function reloadEmail() {
  try {
    ;[emailState.value, emailDeliveries.value] = await Promise.all([
      api.getEmailState(),
      api.listEmailDeliveries(),
    ])
  } catch (err) {
    handle(err)
  }
}

async function reloadBundles() {
  try {
    bundles.value = await api.listBundles()
  } catch (err) {
    handle(err)
  }
}

async function reloadNotifications() {
  try {
    notificationTargets.value = await api.listNotificationTargets()
  } catch (err) {
    handle(err)
  }
}

async function reloadTeam() {
  try {
    employees.value = await api.listEmployees()
  } catch (err) {
    handle(err)
  }
}

async function saveCustomPrices() {
  savingPrices.value = true
  try {
    const num = (v) => (v === '' || v === null ? null : Number(v))
    await api.updateSettings(
      Object.fromEntries(CUSTOM_PRICE_KEYS.map((k) => [k, num(customPrices[k])])),
    )
    flash('Saved custom-blend prices.')
  } catch (err) {
    handle(err)
  } finally {
    savingPrices.value = false
  }
}

async function refreshIntegration() {
  try {
    ;[sync.value, square.value, squareEvents.value] = await Promise.all([
      api.getSyncStatus(),
      api.getSquareStatus(),
      api.listSquareEvents(),
    ])
  } catch (err) {
    handle(err)
  }
}

function money(cents) {
  const sign = cents < 0 ? '-' : ''
  return `${sign}$${(Math.abs(cents) / 100).toFixed(2)}`
}

function shortId(id) {
  return id ? String(id).slice(0, 8) : '—'
}

async function addIngredient(name, type) {
  try {
    ingredients.value = [...ingredients.value, await api.createIngredient(name, type)]
    flash(`Added ingredient “${name}”.`)
  } catch (err) {
    handle(err)
  }
}
async function toggleIngredient(item) {
  try {
    const updated = await api.updateIngredient(item.id, { active: !item.active })
    ingredients.value = ingredients.value.map((i) => (i.id === updated.id ? updated : i))
  } catch (err) {
    handle(err)
  }
}
async function setIngredientType(item, type) {
  try {
    const updated = await api.updateIngredient(item.id, { type })
    ingredients.value = ingredients.value.map((i) => (i.id === updated.id ? updated : i))
  } catch (err) {
    handle(err)
  }
}
async function addScent(name) {
  try {
    scents.value = [...scents.value, await api.createScent(name)]
    flash(`Added scent “${name}”. Open “Formula” to set its ingredients.`)
  } catch (err) {
    handle(err)
  }
}
async function toggleScent(item) {
  try {
    const updated = await api.updateScent(item.id, { active: !item.active })
    scents.value = scents.value.map((s) => (s.id === updated.id ? updated : s))
  } catch (err) {
    handle(err)
  }
}
async function saveScentFormula(scent, items, prices) {
  try {
    const payload = items.map((i) => ({ ingredient_id: i.ingredient_id, amount_ml: Number(i.amount_ml) }))
    const updated = await api.updateScent(scent.id, { items: payload, prices })
    scents.value = scents.value.map((s) => (s.id === updated.id ? updated : s))
    flash(`Saved “${scent.name}”.`)
  } catch (err) {
    handle(err)
  }
}

async function retrySync() {
  try {
    const { requeued } = await api.retrySync()
    flash(requeued ? `Requeued ${requeued} failed sync(s).` : 'No failed syncs to retry.')
    await refreshIntegration()
  } catch (err) {
    handle(err)
  }
}

let flashTimer
function flash(msg) {
  notice.value = msg
  clearTimeout(flashTimer)
  flashTimer = setTimeout(() => (notice.value = ''), 4000)
}

const backingUp = ref(false)
async function backup() {
  backingUp.value = true
  try {
    const { blob, filename } = await downloadBackup()
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = filename
    document.body.appendChild(a)
    a.click()
    a.remove()
    URL.revokeObjectURL(url)
    flash(`Downloaded ${filename}.`)
  } catch (err) {
    handle(err)
  } finally {
    backingUp.value = false
  }
}

const failedCount = computed(() => sync.value?.counts?.failed ?? 0)

function formatTime(value) {
  return value ? new Date(value).toLocaleString() : '—'
}
</script>

<template>
  <p class="error" v-if="error">{{ error }}</p>
  <p class="notice" v-if="notice">{{ notice }}</p>
  <p class="muted" v-if="loading">Loading…</p>

  <template v-else>
    <div class="tabstrip" role="tablist" aria-label="Admin sections">
      <button
        v-for="(tab, i) in TABS"
        :id="`tab-${tab.id}`"
        :key="tab.id"
        role="tab"
        type="button"
        :aria-selected="activeTab === tab.id"
        :aria-controls="`panel-${tab.id}`"
        :tabindex="activeTab === tab.id ? 0 : -1"
        :class="{ active: activeTab === tab.id }"
        @click="selectTab(tab.id)"
        @keydown="onTabKey($event, i)"
      >
        {{ tab.label }}
        <span class="dot" v-if="tabAlert[tab.id]" aria-label="needs attention"></span>
      </button>
    </div>

    <!-- Panels use v-show, not v-if: switching tabs to check something must not
         throw away a half-finished formula or package on another tab. -->
    <section
      id="panel-catalog"
      role="tabpanel"
      aria-labelledby="tab-catalog"
      v-show="activeTab === 'catalog'"
    >
      <CatalogManager
        title="Ingredients"
        noun="ingredient"
        :items="ingredients"
        :types="INGREDIENT_TYPES"
        @add="addIngredient"
        @toggle="toggleIngredient"
        @set-type="setIngredientType"
      />

      <ScentManager
        :scents="scents"
        :ingredients="ingredients"
        @add="addScent"
        @toggle="toggleScent"
        @save="saveScentFormula"
      />
    </section>

    <section
      id="panel-pricing"
      role="tabpanel"
      aria-labelledby="tab-pricing"
      v-show="activeTab === 'pricing'"
    >
    <div class="card">
      <h2>Custom blend pricing</h2>
      <p class="muted">Retail price per size for bespoke custom blends — applies to every custom blend.</p>
      <div class="row">
        <div v-for="size in BOTTLE_SIZES" :key="size.value">
          <label :for="`custom-price-${size.value}`">{{ size.label }}</label>
          <input
            :id="`custom-price-${size.value}`"
            v-model="customPrices[`custom_price_${size.value}`]"
            type="number"
            inputmode="decimal"
            min="0"
            step="0.01"
          />
        </div>
      </div>
      <p class="muted field-help">
        Leave a size blank to not sell custom blends in it — a blank price takes
        the option off the share page rather than offering a bottle with no cost.
      </p>
      <button class="ghost" type="button" :disabled="savingPrices" style="margin-top: 0.8rem" @click="saveCustomPrices">
        {{ savingPrices ? 'Saving…' : 'Save custom-blend prices' }}
      </button>
    </div>

    <div class="card">
      <h2>Referrals</h2>
      <p class="muted">
        When a customer shares a scent from their portal, the link carries their
        code. A friend who buys through it saves — and once that purchase is
        <em>paid</em>, the sharer earns a coupon. Rewards are issued on payment,
        not at checkout, so an abandoned basket never mints one.
      </p>

      <label class="checkbox" style="margin-bottom: 0.8rem">
        <input type="checkbox" v-model="referral.referral_enabled" />
        Referral discounts are running
      </label>

      <div class="row">
        <div class="field grow">
          <label>Buyer saves ($)</label>
          <input v-model="referral.referral_discount_cents" type="number" min="0" step="0.01" />
        </div>
        <div class="field grow">
          <label>Sharer earns ($)</label>
          <input v-model="referral.referral_reward_cents" type="number" min="0" step="0.01" />
        </div>
        <div class="field grow">
          <label>Coupon expires (days)</label>
          <input v-model="referral.coupon_expiry_days" type="number" min="0" step="1" />
        </div>
      </div>
      <p class="muted field-help">
        0 days means coupons never expire. A coupon keeps the value it was issued
        at, so changing these amounts never re-values one already in someone's
        hands. A discount is capped at the order total — it can reduce a sale to
        zero but never below.
      </p>

      <button
        class="ghost"
        type="button"
        :disabled="savingReferral"
        @click="saveReferral"
      >
        {{ savingReferral ? 'Saving…' : 'Save referral settings' }}
      </button>
    </div>

      <BundleManager :bundles="bundles" :scents="scents" @changed="reloadBundles" />
    </section>

    <section
      id="panel-shop"
      role="tabpanel"
      aria-labelledby="tab-shop"
      v-show="activeTab === 'shop'"
    >
      <ShopManager
        :scents="scents"
        @scent-updated="(u) => (scents = scents.map((s) => (s.id === u.id ? u : s)))"
      />
    </section>

    <section
      id="panel-team"
      role="tabpanel"
      aria-labelledby="tab-team"
      v-show="activeTab === 'team'"
    >
      <TeamManager
        :employees="employees"
        :current-email="currentUser?.email || ''"
        @changed="reloadTeam"
      />

      <div class="card">
        <h2>Sign-in security</h2>
        <p class="muted">
          After someone completes MFA, that device is remembered for this long.
          Signing in again from it inside the window needs the password only —
          no authenticator code. The password is <em>always</em> required; this
          never skips it.
        </p>

        <div class="row">
          <div class="field grow">
            <label for="mfa-trust">Don't ask for MFA again for (hours)</label>
            <input
              id="mfa-trust"
              v-model="security.mfa_trust_hours"
              type="number"
              inputmode="numeric"
              min="0"
              max="168"
              step="1"
            />
          </div>
        </div>
        <p class="muted field-help">
          0 asks for a code on every sign-in. The maximum is 168 hours (one week).
          Lowering this applies straight away, including to devices already
          trusted. A device stops being trusted early if the employee's password
          is changed or reset, their MFA is reset, or their account is
          deactivated.
        </p>

        <button class="ghost" type="button" :disabled="savingSecurity" @click="saveSecurity">
          {{ savingSecurity ? 'Saving…' : 'Save sign-in security' }}
        </button>
      </div>
    </section>

    <section
      id="panel-billing"
      role="tabpanel"
      aria-labelledby="tab-billing"
      v-show="activeTab === 'billing'"
    >
    <div class="card">
      <h2>Square billing</h2>

      <dl class="summary" v-if="square">
        <dt>Payments</dt>
        <dd>
          <span class="badge" :class="square.live ? 'ok-badge' : 'danger-badge'">
            {{ square.live ? `Live — ${square.backend}` : 'Mock — no cards are charged' }}
          </span>
        </dd>
        <dt>Webhook receiver</dt>
        <dd>
          <span class="badge" :class="square.webhook_receiver_enabled ? 'ok-badge' : ''">
            {{ square.webhook_receiver_enabled ? 'Enabled' : 'Disabled (no signature key)' }}
          </span>
        </dd>
        <dt>Carts</dt>
        <dd>
          {{ square.cart_counts.paid }} paid · {{ square.cart_counts.pending_payment }} awaiting
          · {{ square.cart_counts.open }} open · {{ square.cart_counts.canceled }} canceled
          <template v-if="square.cart_counts.refunded">
            · <strong class="danger-text">{{ square.cart_counts.refunded }} refunded</strong>
          </template>
        </dd>
        <dt>Awaiting payment</dt>
        <dd>{{ money(square.pending_payment_cents) }}</dd>
        <dt>Contact sync</dt>
        <dd v-if="sync">
          {{ sync.counts.pending }} pending · {{ sync.counts.succeeded }} done ·
          <strong v-if="failedCount" class="danger-text">{{ failedCount }} failed</strong>
          <template v-else>0 failed</template>
        </dd>
      </dl>

      <p class="muted" v-if="square && !square.live">
        Set <code>SQUARE_ACCESS_TOKEN</code> and <code>SQUARE_LOCATION_ID</code> in the
        server’s <code>.env</code> (plus <code>SQUARE_ENV=sandbox</code> while testing)
        and restart to take real payments. Add
        <code>SQUARE_WEBHOOK_SIGNATURE_KEY</code> and <code>SQUARE_WEBHOOK_URL</code>
        so Square can report payments back automatically. These are server-side
        secrets and can’t be set from here.
      </p>

      <div class="row" style="margin-top: 0.75rem">
        <button class="ghost" type="button" style="flex: none" @click="refreshIntegration">
          Refresh
        </button>
        <button
          class="ghost"
          type="button"
          style="flex: none"
          :disabled="!failedCount"
          @click="retrySync"
        >
          Retry failed contact syncs
        </button>
      </div>
    </div>

    <TerminalManager :backend="square?.backend || ''" />

    <div class="card">
      <h2>Recent Square events</h2>
      <p class="muted" v-if="!squareEvents.length">
        No webhooks received yet. Until the receiver is configured, use “Check Square”
        on a checkout to pull the result instead.
      </p>
      <div v-for="event in squareEvents" :key="event.id" class="list-item" style="cursor: default">
        <span class="grow">
          <strong>{{ event.event_type }}</strong>
          <span class="muted">
            {{ event.square_payment_id || event.square_order_id || '—' }} ·
            {{ formatTime(event.received_at) }}
          </span>
          <span class="muted danger-text" v-if="event.error">{{ event.error }}</span>
        </span>
        <span class="badge" :class="event.status === 'processed' ? 'ok-badge' : ''">
          {{ event.status }}
        </span>
      </div>
    </div>
    </section>

    <section
      id="panel-alerts"
      role="tabpanel"
      aria-labelledby="tab-alerts"
      v-show="activeTab === 'alerts'"
    >
      <NotificationManager :targets="notificationTargets" @changed="reloadNotifications" />
    </section>

    <section
      id="panel-enquiries"
      role="tabpanel"
      aria-labelledby="tab-enquiries"
      v-show="activeTab === 'enquiries'"
    >
      <EventEnquiries @changed="(n) => (openEnquiries = n)" />
    </section>

    <section
      id="panel-email"
      role="tabpanel"
      aria-labelledby="tab-email"
      v-show="activeTab === 'email'"
    >
      <EmailManager :state="emailState" :deliveries="emailDeliveries" @changed="reloadEmail" />
    </section>

    <section
      id="panel-data"
      role="tabpanel"
      aria-labelledby="tab-data"
      v-show="activeTab === 'data'"
    >
    <BackupScheduler
      :status="backupStatus"
      :destinations="backupDestinations"
      :runs="backupRuns"
      @changed="reloadBackups"
    />

    <div class="card">
      <h2>Manual backup</h2>
      <p class="muted">
        Download a full database backup (SQL) right now — useful for taking a
        snapshot before a risky change. It is <strong>not encrypted</strong> and it
        contains all customer data, so store it somewhere safe and delete it when
        you’re done. Protection against losing this server is the schedule above,
        not this button.
      </p>
      <button class="ghost" type="button" :disabled="backingUp" @click="backup">
        {{ backingUp ? 'Preparing…' : 'Download database backup' }}
      </button>
    </div>

    <!-- Last on the tab on purpose: the destructive control should not sit above
         the ones people use every day. -->
    <BackupRestore />
    </section>

    <section
      id="panel-activity"
      role="tabpanel"
      aria-labelledby="tab-activity"
      v-show="activeTab === 'activity'"
    >
      <!-- Loads its own data. It paginates, filters and can be re-queried
           independently, so threading it through the view's single load() would
           only couple the two. -->
      <AuditLog />
    </section>
  </template>
</template>

<style scoped>
/* Sticky so the tabs stay reachable inside a long panel — Billing in
   particular can run to several screens. */
.tabstrip {
  position: sticky;
  top: 0;
  z-index: 5;
  display: flex;
  gap: 0.25rem;
  margin-bottom: 1rem;
  padding: 0.5rem 0;
  background: var(--bg);
  border-bottom: 1px solid var(--border);
  /* Wrap onto a second row when the tabs won't fit — they used to scroll
     horizontally with the scrollbar hidden, which silently cut "Data" off the
     right edge on a tablet. A tab nobody can see is a feature nobody can use;
     the row of extra height costs less than that. */
  flex-wrap: wrap;
}

.tabstrip button {
  flex: none;
  position: relative;
  padding: 0.6rem 0.95rem;
  min-height: var(--tap);
  border: 1px solid transparent;
  border-radius: var(--radius);
  background: none;
  color: var(--muted);
  font: inherit;
  font-size: 0.82rem;
  font-weight: 600;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  white-space: nowrap;
  cursor: pointer;
}

.tabstrip button:hover {
  color: var(--ink);
  background: var(--surface-alt);
}

.tabstrip button.active {
  color: var(--accent-deep);
  background: var(--surface);
  border-color: var(--border);
}

.tabstrip button:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 1px;
}

/* A problem must not be hidden by the grouping that tidied the page up. */
.dot {
  display: inline-block;
  width: 7px;
  height: 7px;
  margin-left: 0.4rem;
  border-radius: 50%;
  background: var(--danger);
  vertical-align: middle;
}
</style>
