<script setup>
/**
 * Scheduled, encrypted, off-box backups.
 *
 * The manual download button beside this one is a *pull* — it protects against
 * "I'm about to change something risky" and not at all against "the VPS is
 * gone". This panel is the automatic half.
 *
 * Two things this page deliberately does NOT do:
 *
 *   - Show the passphrase back. It can be set, never read. An endpoint that
 *     returned it would turn any admin session into a copy of the decryption
 *     key.
 *   - Let a destination be saved that cannot work. The schedule, the recipient
 *     and the retention count are all checked by the server on save, because the
 *     alternative is finding out from a red row at 3am — by which point that
 *     night's backup has already not happened.
 */
import { computed, reactive, ref } from 'vue'

import { api } from '../lib/api.js'

const props = defineProps({
  status: { type: Object, default: null },
  destinations: { type: Array, default: () => [] },
  runs: { type: Array, default: () => [] },
})
const emit = defineEmits(['changed'])

const busy = ref('')
const error = ref('')
const notice = ref('')

const passphrase = ref('')
const confirmPassphrase = ref('')

const showAdd = ref(false)
const form = reactive({
  label: '',
  kind: 'google_drive',
  frequency: 'daily',
  // 3:30am, not 2am: 2am does not exist on the spring-forward day, and 1:30am
  // happens twice on the autumn one. 3:30 is unambiguous all year.
  time: '03:30',
  weekday: '1',
  everyHours: '6',
  cron: '30 3 * * *',
  timezone: 'America/Chicago',
  retain_count: 30,
  to: '',
  folder_id: '',
  impersonate: '',
})

const ZONES = [
  'America/Chicago',
  'America/New_York',
  'America/Denver',
  'America/Los_Angeles',
  'UTC',
]

const WEEKDAYS = [
  ['1', 'Monday'],
  ['2', 'Tuesday'],
  ['3', 'Wednesday'],
  ['4', 'Thursday'],
  ['5', 'Friday'],
  ['6', 'Saturday'],
  ['0', 'Sunday'],
]

const passphraseSet = computed(() => Boolean(props.status?.passphrase_set))
const envManaged = computed(() => Boolean(props.status?.passphrase_env_managed))
const googleConnected = computed(() => Boolean(props.status?.google_connected))
const emailLive = computed(() => Boolean(props.status?.email_live))

/**
 * The presets build a standard 5-field cron expression, which is also what the
 * server stores. Shown to the user rather than hidden: someone who knows cron
 * can check it, and someone who doesn't has the plain-English line above it.
 */
const cronExpression = computed(() => {
  const [h, m] = form.time.split(':')
  const hour = Number(h)
  const minute = Number(m)
  switch (form.frequency) {
    case 'hourly':
      return '0 * * * *'
    case 'everyN':
      return `0 */${Number(form.everyHours) || 6} * * *`
    case 'daily':
      return `${minute} ${hour} * * *`
    case 'weekly':
      return `${minute} ${hour} * * ${form.weekday}`
    default:
      return form.cron
  }
})

const describedSchedule = computed(() => {
  const expr = cronExpression.value
  switch (form.frequency) {
    case 'hourly':
      return 'Every hour, on the hour'
    case 'everyN':
      return `Every ${form.everyHours} hours`
    case 'daily':
      return `Every day at ${prettyTime(form.time)}`
    case 'weekly': {
      const day = WEEKDAYS.find(([v]) => v === form.weekday)?.[1] ?? ''
      return `Every ${day} at ${prettyTime(form.time)}`
    }
    default:
      return expr
  }
})

function prettyTime(hhmm) {
  const [h, m] = hhmm.split(':').map(Number)
  const suffix = h < 12 ? 'am' : 'pm'
  const display = h % 12 === 0 ? 12 : h % 12
  return `${display}:${String(m).padStart(2, '0')}${suffix}`
}

function formatWhen(value) {
  return value ? new Date(value).toLocaleString() : '—'
}

function formatBytes(n) {
  if (n == null) return '—'
  if (n >= 1048576) return `${(n / 1048576).toFixed(1)} MB`
  if (n >= 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${n} bytes`
}

function describeStored(dest) {
  const f = dest.schedule.trim().split(/\s+/)
  if (f.length !== 5) return dest.schedule
  const [min, hour, dom, mon, dow] = f
  if (dom !== '*' || mon !== '*') return dest.schedule
  if (hour === '*') return min === '0' ? 'Hourly, on the hour' : `Hourly, at ${min} past`
  if (hour.startsWith('*/')) return `Every ${hour.slice(2)} hours`
  const time = prettyTime(`${String(hour).padStart(2, '0')}:${String(min).padStart(2, '0')}`)
  if (dow === '*') return `Daily at ${time}`
  const day = WEEKDAYS.find(([v]) => v === dow)?.[1]
  return day ? `Every ${day} at ${time}` : dest.schedule
}

function destinationName(dest) {
  if (dest.kind === 'google_drive') return 'Google Drive'
  if (dest.kind === 'email') return `Email → ${dest.config?.to ?? '?'}`
  if (dest.kind === 'sharepoint') return 'SharePoint'
  return dest.kind
}

async function savePassphrase() {
  if (passphrase.value !== confirmPassphrase.value) {
    error.value = 'The two passphrases do not match.'
    return
  }
  if (passphrase.value.trim().length < 12) {
    error.value = 'Use at least 12 characters.'
    return
  }
  busy.value = 'passphrase'
  error.value = ''
  notice.value = ''
  try {
    const res = await api.setBackupPassphrase(passphrase.value)
    notice.value = res.note
    passphrase.value = ''
    confirmPassphrase.value = ''
    emit('changed')
  } catch (e) {
    error.value = e.message
  } finally {
    busy.value = ''
  }
}

async function addDestination() {
  busy.value = 'add'
  error.value = ''
  notice.value = ''
  try {
    const config = {}
    if (form.kind === 'email') config.to = form.to.trim()
    if (form.kind === 'google_drive') {
      if (form.folder_id.trim()) config.folder_id = form.folder_id.trim()
      if (form.impersonate.trim()) config.impersonate = form.impersonate.trim()
    }
    await api.createBackupDestination({
      label: form.label.trim() || destinationName({ kind: form.kind, config }),
      kind: form.kind,
      config,
      schedule: cronExpression.value,
      timezone: form.timezone,
      retain_count: Number(form.retain_count),
    })
    showAdd.value = false
    form.label = ''
    form.to = ''
    form.folder_id = ''
    form.impersonate = ''
    notice.value = 'Backup scheduled. Use “Run now” to prove it works before relying on it.'
    emit('changed')
  } catch (e) {
    error.value = e.message
  } finally {
    busy.value = ''
  }
}

async function runNow(dest) {
  busy.value = `run-${dest.id}`
  error.value = ''
  notice.value = ''
  try {
    const res = await api.runBackupNow(dest.id)
    notice.value = `Sent ${res.filename} (${formatBytes(res.bytes)} encrypted).`
    emit('changed')
  } catch (e) {
    error.value = e.message
    emit('changed')
  } finally {
    busy.value = ''
  }
}

async function toggle(dest) {
  busy.value = `toggle-${dest.id}`
  error.value = ''
  try {
    await api.updateBackupDestination(dest.id, { enabled: !dest.enabled })
    emit('changed')
  } catch (e) {
    error.value = e.message
  } finally {
    busy.value = ''
  }
}

async function remove(dest) {
  if (
    !confirm(
      `Stop backing up to “${dest.label}”?\n\nBackups already sent there are left alone — ` +
        `this only cancels future ones.`,
    )
  ) {
    return
  }
  busy.value = `del-${dest.id}`
  error.value = ''
  try {
    await api.deleteBackupDestination(dest.id)
    emit('changed')
  } catch (e) {
    error.value = e.message
  } finally {
    busy.value = ''
  }
}
</script>

<template>
  <div class="card">
    <h2>Scheduled backups</h2>

    <p class="error" v-if="error">{{ error }}</p>
    <p class="notice" v-if="notice">{{ notice }}</p>

    <dl class="summary" v-if="props.status">
      <dt>Encryption</dt>
      <dd>
        <span class="badge" :class="passphraseSet ? 'ok-badge' : 'danger-badge'">
          {{ passphraseSet ? 'Passphrase set' : 'No passphrase — nothing will run' }}
        </span>
      </dd>
      <dt>Last successful backup</dt>
      <dd>
        <span
          class="badge"
          :class="props.status.last_success_at ? 'ok-badge' : 'danger-badge'"
        >
          {{ props.status.last_success_at ? formatWhen(props.status.last_success_at) : 'Never' }}
        </span>
      </dd>
      <dt>Schedules</dt>
      <dd>{{ props.status.enabled_destinations }} active</dd>
    </dl>

    <!-- Encryption comes first because nothing else works without it. -->
    <template v-if="!passphraseSet">
      <p class="muted danger-text">
        Backups are always encrypted, so nothing runs until a passphrase is set.
      </p>
      <p class="muted" v-if="envManaged">
        The passphrase is set in the server’s <code>.env</code>
        (<code>BACKUP_PASSPHRASE</code>), which takes precedence over anything set
        here.
      </p>
      <template v-else>
        <div class="row">
          <div class="field">
            <label>Backup passphrase</label>
            <input v-model="passphrase" type="password" autocomplete="new-password" />
          </div>
          <div class="field">
            <label>Confirm</label>
            <input v-model="confirmPassphrase" type="password" autocomplete="new-password" />
          </div>
        </div>
        <p class="muted field-help">
          <strong>Write this down and store it somewhere that is not this server.</strong>
          It is the only thing that can decrypt the backups — if it is lost, they
          are unrecoverable, and there is no way for us to reset it. It is also the
          only thing standing between a leaked backup file and every customer
          record in the business.
        </p>
        <button
          class="primary"
          type="button"
          :disabled="busy === 'passphrase'"
          @click="savePassphrase"
        >
          {{ busy === 'passphrase' ? 'Saving…' : 'Set passphrase' }}
        </button>
      </template>
    </template>

    <!-- --- Destinations --------------------------------------------------- -->
    <template v-else>
      <table class="dest-table" v-if="props.destinations.length">
        <thead>
          <tr>
            <th>Where</th>
            <th>When</th>
            <th>Next</th>
            <th>Last run</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="d in props.destinations" :key="d.id" :class="{ off: !d.enabled }">
            <td>
              <strong>{{ d.label }}</strong>
              <span class="sub">{{ destinationName(d) }}</span>
            </td>
            <td>
              {{ describeStored(d) }}
              <span class="sub">{{ d.timezone }} · keep {{ d.retain_count }}</span>
            </td>
            <td>{{ d.enabled ? formatWhen(d.next_run_at) : 'Paused' }}</td>
            <td>
              <span
                v-if="d.last_status"
                class="badge"
                :class="d.last_status === 'ok' ? 'ok-badge' : 'danger-badge'"
              >
                {{ d.last_status === 'ok' ? 'OK' : 'Failed' }}
              </span>
              <span v-else class="sub">never run</span>
              <span class="sub">{{ formatWhen(d.last_run_at) }}</span>
              <span class="sub danger-text" v-if="d.last_error">{{ d.last_error }}</span>
            </td>
            <td class="actions">
              <button
                class="ghost"
                type="button"
                :disabled="busy === `run-${d.id}`"
                @click="runNow(d)"
              >
                {{ busy === `run-${d.id}` ? 'Running…' : 'Run now' }}
              </button>
              <button class="ghost" type="button" @click="toggle(d)">
                {{ d.enabled ? 'Pause' : 'Resume' }}
              </button>
              <button class="ghost" type="button" @click="remove(d)">Remove</button>
            </td>
          </tr>
        </tbody>
      </table>

      <p class="muted danger-text" v-else>
        No backups are scheduled. The database exists only on this server.
      </p>

      <button class="ghost" type="button" @click="showAdd = !showAdd">
        {{ showAdd ? 'Cancel' : 'Add a backup schedule' }}
      </button>

      <div class="add-form" v-if="showAdd">
        <div class="row">
          <div class="field">
            <label>Send to</label>
            <select v-model="form.kind">
              <option value="google_drive">Google Drive</option>
              <option value="email">Email</option>
              <option value="sharepoint" disabled>SharePoint (not available)</option>
            </select>
          </div>
          <div class="field">
            <label>Name</label>
            <input v-model="form.label" type="text" placeholder="e.g. Nightly to Drive" />
          </div>
        </div>

        <!-- Drive -->
        <template v-if="form.kind === 'google_drive'">
          <p class="muted danger-text" v-if="!googleConnected">
            Google isn’t connected. Connect a service-account key under
            <strong>Admin → Email</strong> first — Drive backups reuse that key.
          </p>
          <div class="row">
            <div class="field">
              <label>Upload as (Workspace mailbox)</label>
              <input
                v-model="form.impersonate"
                type="email"
                placeholder="leave blank to use the email sender"
              />
            </div>
            <div class="field">
              <label>Folder ID (optional)</label>
              <input v-model="form.folder_id" type="text" placeholder="from the Drive folder URL" />
            </div>
          </div>
          <p class="muted field-help">
            A service account has no Drive of its own, so the upload runs as a real
            Workspace user and the file lands in their Drive. That account’s
            client ID must be authorised for
            <code>https://www.googleapis.com/auth/drive.file</code> under Google
            Admin → Security → API controls → Domain-wide delegation — this is a
            <em>separate</em> entry from the Gmail one, and adding it is the step
            people miss. Leave the folder blank to drop files in My Drive.
          </p>
        </template>

        <!-- Email -->
        <template v-if="form.kind === 'email'">
          <p class="muted danger-text" v-if="!emailLive">
            Email isn’t configured, so nothing can be sent. Set it up under
            <strong>Admin → Email</strong> first.
          </p>
          <div class="field">
            <label>Send to</label>
            <input v-model="form.to" type="email" placeholder="owner@theblendbarokc.com" />
          </div>
          <p class="muted field-help">
            Attachments cap out around 18MB, so this stops working once the
            database outgrows it — Drive is the better primary. Sent mail also
            can’t be deleted, so the retention setting below doesn’t apply here
            and copies build up in the mailbox forever. The attachment is
            encrypted, which is what makes that survivable.
          </p>
        </template>

        <div class="row">
          <div class="field">
            <label>How often</label>
            <select v-model="form.frequency">
              <option value="hourly">Every hour</option>
              <option value="everyN">Every few hours</option>
              <option value="daily">Every day</option>
              <option value="weekly">Every week</option>
              <option value="custom">Custom (cron)</option>
            </select>
          </div>

          <div class="field" v-if="form.frequency === 'everyN'">
            <label>Hours apart</label>
            <input v-model="form.everyHours" type="number" min="1" max="23" />
          </div>

          <div class="field" v-if="form.frequency === 'weekly'">
            <label>Day</label>
            <select v-model="form.weekday">
              <option v-for="[value, name] in WEEKDAYS" :key="value" :value="value">
                {{ name }}
              </option>
            </select>
          </div>

          <div class="field" v-if="form.frequency === 'daily' || form.frequency === 'weekly'">
            <label>Time</label>
            <input v-model="form.time" type="time" />
          </div>

          <div class="field" v-if="form.frequency === 'custom'">
            <label>Cron (5 fields)</label>
            <input v-model="form.cron" type="text" placeholder="30 3 * * *" spellcheck="false" />
          </div>
        </div>

        <div class="row">
          <div class="field">
            <label>Timezone</label>
            <select v-model="form.timezone">
              <option v-for="z in ZONES" :key="z" :value="z">{{ z }}</option>
            </select>
          </div>
          <div class="field">
            <label>Keep the last</label>
            <input v-model="form.retain_count" type="number" min="1" max="3650" />
          </div>
        </div>
        <p class="muted field-help">
          Older backups beyond that count are deleted from Drive after each
          successful run — an hourly schedule is about 8,760 files a year
          otherwise. Only files this scheduler uploaded are ever removed.
        </p>

        <p class="schedule-preview">
          <strong>{{ describedSchedule }}</strong>
          <span class="sub">{{ form.timezone }} · <code>{{ cronExpression }}</code></span>
        </p>

        <button class="primary" type="button" :disabled="busy === 'add'" @click="addDestination">
          {{ busy === 'add' ? 'Saving…' : 'Schedule it' }}
        </button>
      </div>

      <!-- --- History ------------------------------------------------------ -->
      <template v-if="props.runs.length">
        <h3 class="runs-heading">Recent runs</h3>
        <table class="dest-table">
          <thead>
            <tr>
              <th>When</th>
              <th>Where</th>
              <th>Result</th>
              <th>Size</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="r in props.runs" :key="r.id">
              <td>
                {{ formatWhen(r.started_at) }}
                <span class="sub" v-if="r.trigger === 'manual'">manual</span>
              </td>
              <td>{{ r.destination_label }}</td>
              <td>
                <span
                  class="badge"
                  :class="r.status === 'ok' ? 'ok-badge' : r.status === 'running' ? '' : 'danger-badge'"
                >
                  {{ r.status === 'ok' ? 'OK' : r.status === 'running' ? 'Running' : 'Failed' }}
                </span>
                <span class="sub danger-text" v-if="r.error">{{ r.error }}</span>
              </td>
              <td>{{ formatBytes(r.bytes) }}</td>
            </tr>
          </tbody>
        </table>
      </template>

      <p class="muted restore-note">
        Backups are gzipped, encrypted with <code>age</code>, and named
        <code>blendbar-backup-*.sql.gz.age</code>. To restore one you need the
        passphrase and the <code>age</code> tool — this app is not required, which
        is the point:
      </p>
      <pre class="restore">age -d blendbar-backup-….sql.gz.age | gunzip | psql "$DATABASE_URL"</pre>
    </template>
  </div>
</template>

<style scoped>
.dest-table {
  width: 100%;
  border-collapse: collapse;
  margin: 0.8rem 0 1rem;
  font-size: 0.9rem;
}

.dest-table th {
  text-align: left;
  font-size: 0.7rem;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--muted);
  font-weight: 600;
  padding: 0 0.6rem 0.4rem 0;
  border-bottom: 1px solid var(--border);
}

.dest-table td {
  padding: 0.7rem 0.6rem 0.7rem 0;
  border-bottom: 1px solid var(--border);
  vertical-align: top;
}

/* A paused schedule is still a row, but it must not read as a working one. */
.dest-table tr.off {
  opacity: 0.55;
}

/* Secondary detail under the main cell value. Block so each lands on its own
   line — an error message inline with a timestamp is unreadable. */
.sub {
  display: block;
  font-size: 0.78rem;
  color: var(--muted);
  margin-top: 0.15rem;
}

.actions {
  white-space: nowrap;
}

.actions button {
  margin: 0 0.2rem 0.2rem 0;
}

.add-form {
  margin-top: 1rem;
  padding-top: 1rem;
  border-top: 1px solid var(--border);
}

/* The literal cron expression, so what was chosen from the dropdowns is visible
   and checkable rather than hidden behind them. */
.schedule-preview {
  margin: 0.6rem 0 1rem;
  padding: 0.7rem 0.9rem;
  background: var(--surface-alt);
  border-radius: var(--radius);
  font-size: 0.9rem;
}

.runs-heading {
  margin: 1.6rem 0 0;
  font-size: 0.95rem;
}

.restore-note {
  margin-top: 1.4rem;
}

.restore {
  margin: 0.4rem 0 0;
  padding: 0.7rem 0.9rem;
  background: var(--surface-alt);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  font-size: 0.78rem;
  overflow-x: auto;
  white-space: pre;
}
</style>
