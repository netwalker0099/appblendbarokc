<script setup>
/**
 * The activity log.
 *
 * Read-only, and there is no control here that could be mistaken for an edit —
 * no delete, no "clear old entries", nothing. That is the point of the feature:
 * an audit log with a delete button is a suggestion, not a record.
 *
 * The Verify button is the part that gives the log its value. Entries are
 * hash-chained, so altering or removing one breaks every hash after it — but
 * tamper-evidence is worth exactly nothing unless somebody checks, and a button
 * is the only way that ever happens.
 */
import { onMounted, reactive, ref } from 'vue'

import { api } from '../lib/api.js'

const entries = ref([])
const total = ref(0)
const loading = ref(false)
const error = ref('')
const expanded = ref(new Set())

const chain = ref(null)
const verifying = ref(false)

const segments = ref([])
const retention = ref(0)
const savingRetention = ref(false)
const archiving = ref(false)
const notice = ref('')

const filters = reactive({
  actor: '',
  // Defaults to admin actions: the log records every employee mutation, and
  // showing worker intake by default would bury the entries someone came here
  // looking for.
  role: 'admin',
  path: '',
  failures_only: false,
})

const PAGE = 100
const offset = ref(0)

onMounted(() => {
  load(true)
  loadRetention()
})

async function loadRetention() {
  try {
    const [settings, segs] = await Promise.all([api.getSettings(), api.listAuditSegments()])
    retention.value = settings.audit_retention_days ?? 0
    segments.value = segs
  } catch (e) {
    error.value = e.message
  }
}

async function saveRetention() {
  savingRetention.value = true
  error.value = ''
  notice.value = ''
  try {
    await api.updateSettings({ audit_retention_days: Number(retention.value) })
    notice.value =
      Number(retention.value) === 0
        ? 'Retention is off — every entry is kept in the database.'
        : `Entries older than ${retention.value} days will be archived off-box, then removed.`
    await loadRetention()
  } catch (e) {
    error.value = e.message
  } finally {
    savingRetention.value = false
  }
}

async function archiveNow() {
  archiving.value = true
  error.value = ''
  notice.value = ''
  try {
    const res = await api.archiveAuditNow()
    notice.value = res.archived
      ? `Archived ${res.entry_count} entries as ${res.filename}, delivered to ${res.delivered_to.join(', ')}.`
      : res.reason
    await Promise.all([load(true), loadRetention()])
  } catch (e) {
    // Retention refuses to prune what it could not deliver, so the error IS the
    // useful outcome here — it says why nothing was removed.
    error.value = e.message
  } finally {
    archiving.value = false
  }
}

async function load(reset = false) {
  loading.value = true
  error.value = ''
  if (reset) offset.value = 0
  try {
    const res = await api.listAuditLog({ ...filters, limit: PAGE, offset: offset.value })
    entries.value = reset ? res.entries : [...entries.value, ...res.entries]
    total.value = res.total
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}

async function loadMore() {
  offset.value += PAGE
  await load(false)
}

async function verify() {
  verifying.value = true
  error.value = ''
  try {
    chain.value = await api.verifyAuditChain()
  } catch (e) {
    error.value = e.message
  } finally {
    verifying.value = false
  }
}

function toggle(id) {
  const next = new Set(expanded.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  expanded.value = next
}

function formatWhen(value) {
  return value ? new Date(value).toLocaleString() : '—'
}

function statusClass(status) {
  if (status >= 500) return 'danger-badge'
  if (status >= 400) return 'warn-badge'
  return 'ok-badge'
}

function hasDetail(entry) {
  return entry.detail && Object.keys(entry.detail).length > 0
}
</script>

<template>
  <div class="card">
    <h2>Activity log</h2>

    <p class="error" v-if="error">{{ error }}</p>
    <p class="notice" v-if="notice">{{ notice }}</p>

    <p class="muted">
      Every change made by a signed-in member of staff, plus database downloads.
      Entries cannot be edited or deleted — the database refuses, and each one is
      hash-chained to the one before it, so altering history breaks every entry
      after it.
    </p>

    <!-- Chain integrity -->
    <div class="chain">
      <button class="ghost" type="button" :disabled="verifying" @click="verify">
        {{ verifying ? 'Checking…' : 'Verify the log has not been tampered with' }}
      </button>

      <template v-if="chain">
        <p v-if="chain.intact" class="chain-result">
          <span class="badge ok-badge">Intact</span>
          All {{ chain.entries_checked }} entries verified.
        </p>
        <p v-else class="chain-result">
          <span class="badge danger-badge">TAMPERED</span>
          {{ chain.breaks.length }} broken
          {{ chain.breaks.length === 1 ? 'entry' : 'entries' }} out of
          {{ chain.entries_checked }}.
        </p>
        <ul class="breaks" v-if="!chain.intact">
          <li v-for="b in chain.breaks" :key="b.id">#{{ b.id }} — {{ b.reason }}</li>
        </ul>
        <p class="muted chain-head" v-if="chain.head">
          Current chain head: <code>{{ chain.head.slice(0, 32) }}…</code>
          <br />
          Writing this value down somewhere off this server is what would let you
          prove later that nothing was rewritten — including by someone with full
          database access, which the chain alone cannot rule out.
        </p>
      </template>
    </div>

    <!-- Retention -->
    <div class="chain">
      <h3 class="sub-heading">Retention</h3>
      <p class="muted">
        The log only grows. Rather than deleting old entries, anything past the
        window is written to an encrypted archive, sent to your backup
        destinations, and only then removed from the table. If it cannot be
        delivered anywhere, nothing is removed — the table grows instead, which is
        the safe way to fail.
      </p>
      <div class="row">
        <div class="field">
          <label>Keep entries in the database for</label>
          <select v-model="retention">
            <option :value="0">Forever (never archive)</option>
            <option :value="90">90 days</option>
            <option :value="180">180 days</option>
            <option :value="365">1 year</option>
            <option :value="730">2 years</option>
          </select>
        </div>
      </div>
      <button class="ghost" type="button" :disabled="savingRetention" @click="saveRetention">
        {{ savingRetention ? 'Saving…' : 'Save retention' }}
      </button>
      <button
        class="ghost"
        type="button"
        v-if="Number(retention) > 0"
        :disabled="archiving"
        @click="archiveNow"
      >
        {{ archiving ? 'Archiving…' : 'Archive now' }}
      </button>

      <template v-if="segments.length">
        <h4 class="sub-heading">Archived off-box</h4>
        <table class="audit-table">
          <thead>
            <tr>
              <th>Period</th>
              <th>Entries</th>
              <th>File</th>
              <th>Sent to</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="s in segments" :key="s.id">
              <td class="when">
                {{ formatWhen(s.from_at) }}
                <span class="sub">to {{ formatWhen(s.to_at) }}</span>
              </td>
              <td>
                {{ s.entry_count }}
                <span class="sub">#{{ s.from_id }}–{{ s.to_id }}</span>
              </td>
              <td>
                <code class="filename">{{ s.filename }}</code>
                <span class="sub">sha256 {{ s.content_sha256.slice(0, 16) }}…</span>
              </td>
              <td>
                <span v-for="(d, i) in s.destinations" :key="i" class="sub">{{ d.destination }}</span>
              </td>
            </tr>
          </tbody>
        </table>
        <p class="muted">
          These records are permanent and are never pruned — they are what lets the
          chain above still verify across an archived gap. To read one back:
        </p>
        <pre class="restore">age -d blendbar-audit-….jsonl.gz.age | gunzip &gt; segment.jsonl
docker compose exec -T api blendbar-api import-audit-archive /tmp/segment.jsonl</pre>
      </template>
    </div>

    <!-- Filters -->
    <div class="row">
      <div class="field">
        <label>Who</label>
        <input v-model="filters.actor" type="text" placeholder="email" @change="load(true)" />
      </div>
      <div class="field">
        <label>Role</label>
        <select v-model="filters.role" @change="load(true)">
          <option value="admin">Admins</option>
          <option value="worker">Workers</option>
          <option value="">Everyone</option>
        </select>
      </div>
      <div class="field">
        <label>Path contains</label>
        <input v-model="filters.path" type="text" placeholder="e.g. pricing" @change="load(true)" />
      </div>
    </div>
    <label class="checkbox">
      <input v-model="filters.failures_only" type="checkbox" @change="load(true)" />
      Only show attempts that were refused or failed
    </label>

    <table class="audit-table" v-if="entries.length">
      <thead>
        <tr>
          <th>When</th>
          <th>Who</th>
          <th>What</th>
          <th>Result</th>
        </tr>
      </thead>
      <tbody>
        <template v-for="e in entries" :key="e.id">
          <tr :class="{ clickable: hasDetail(e) }" @click="hasDetail(e) && toggle(e.id)">
            <td class="when">{{ formatWhen(e.at) }}</td>
            <td>
              {{ e.actor_email }}
              <span class="sub">{{ e.actor_role }}<template v-if="e.ip"> · {{ e.ip }}</template></span>
            </td>
            <td>
              {{ e.summary }}
              <span class="sub"><code>{{ e.method }} {{ e.path }}</code></span>
            </td>
            <td>
              <span class="badge" :class="statusClass(e.status)">{{ e.status }}</span>
              <span class="sub expand" v-if="hasDetail(e)">
                {{ expanded.has(e.id) ? 'hide' : 'details' }}
              </span>
            </td>
          </tr>
          <tr v-if="expanded.has(e.id)" :key="`${e.id}-detail`">
            <td colspan="4">
              <pre class="detail">{{ JSON.stringify(e.detail, null, 2) }}</pre>
              <p class="sub">
                Secrets are stripped before an entry is stored, so passwords, MFA
                codes and keys read as <code>[redacted]</code>.
                <br />
                Entry hash: <code>{{ e.entry_hash.slice(0, 32) }}…</code>
              </p>
            </td>
          </tr>
        </template>
      </tbody>
    </table>

    <p class="muted" v-else-if="!loading">Nothing recorded yet for this filter.</p>

    <div class="footer">
      <button
        class="ghost"
        type="button"
        v-if="entries.length && entries.length < total"
        :disabled="loading"
        @click="loadMore"
      >
        {{ loading ? 'Loading…' : 'Load more' }}
      </button>
      <span class="muted" v-if="entries.length">
        Showing {{ entries.length }} of {{ total }} recorded actions
      </span>
    </div>
  </div>
</template>

<style scoped>
/* A 4xx is a refused attempt, not a system failure — "someone tried to do this
   and was stopped" is worth seeing, but it should not shout like a 500 does.
   Scoped here because nowhere else in the app distinguishes the two. */
.badge.warn-badge {
  background: color-mix(in srgb, var(--accent) 20%, var(--surface));
  color: var(--accent-deep);
}

.chain {
  margin: 1rem 0 1.4rem;
  padding: 0.9rem 1rem;
  background: var(--surface-alt);
  border-radius: var(--radius);
}

.chain-result {
  margin: 0.7rem 0 0;
  font-size: 0.9rem;
}

.breaks {
  margin: 0.5rem 0 0;
  padding-left: 1.1rem;
  font-size: 0.85rem;
  color: var(--danger);
}

.chain-head {
  margin: 0.7rem 0 0;
  font-size: 0.78rem;
}

.sub-heading {
  margin: 0 0 0.5rem;
  font-size: 0.95rem;
}

h4.sub-heading {
  margin-top: 1.4rem;
  font-size: 0.85rem;
}

.filename {
  font-size: 0.75rem;
  word-break: break-all;
}

.restore {
  margin: 0.4rem 0 0;
  padding: 0.7rem 0.9rem;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  font-size: 0.76rem;
  overflow-x: auto;
  white-space: pre;
}

.audit-table {
  width: 100%;
  border-collapse: collapse;
  margin: 1rem 0 0.6rem;
  font-size: 0.88rem;
}

.audit-table th {
  text-align: left;
  font-size: 0.7rem;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--muted);
  font-weight: 600;
  padding: 0 0.6rem 0.4rem 0;
  border-bottom: 1px solid var(--border);
}

.audit-table td {
  padding: 0.6rem 0.6rem 0.6rem 0;
  border-bottom: 1px solid var(--border);
  vertical-align: top;
}

.audit-table tr.clickable {
  cursor: pointer;
}

.audit-table tr.clickable:hover {
  background: var(--surface-alt);
}

.when {
  white-space: nowrap;
}

.sub {
  display: block;
  font-size: 0.76rem;
  color: var(--muted);
  margin-top: 0.15rem;
}

.expand {
  text-decoration: underline;
}

.detail {
  margin: 0.3rem 0 0.5rem;
  padding: 0.7rem 0.9rem;
  background: var(--surface-alt);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  font-size: 0.78rem;
  overflow-x: auto;
}

.footer {
  display: flex;
  align-items: center;
  gap: 0.9rem;
  margin-top: 0.6rem;
}
</style>
