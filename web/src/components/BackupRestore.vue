<script setup>
/**
 * Restoring the database from a backup file.
 *
 * The most destructive control in the application, so the flow is built to make
 * the destructive step the *last* thing that happens and the hardest to reach:
 *
 *   1. Choose a file — it is uploaded and inspected immediately. The server
 *      decrypts it, loads it into a scratch database and reports what is inside.
 *      Nothing live is touched. If it is the wrong file, that is visible here.
 *   2. Read the contents. Table and row counts, next to what is in the database
 *      right now.
 *   3. Type the confirmation phrase. Not a checkbox — a phrase naming the
 *      consequence, which cannot be agreed to by reflex.
 *
 * The server takes its own encrypted safety copy immediately before overwriting
 * anything, so step 3 is reversible even when it was the wrong file.
 */
import { computed, onMounted, ref } from 'vue'

import { api, uploadBackupForRestore } from '../lib/api.js'

const CONFIRM_PHRASE = 'REPLACE ALL DATA'

const file = ref(null)
const report = ref(null)
const inspecting = ref(false)
const restoring = ref(false)
const typed = ref('')
const error = ref('')
const result = ref(null)
const safetyCopies = ref([])

const canRestore = computed(
  () => report.value && typed.value.trim() === CONFIRM_PHRASE && !restoring.value,
)

onMounted(loadSafetyCopies)

async function loadSafetyCopies() {
  try {
    safetyCopies.value = (await api.listSafetyCopies()).copies
  } catch {
    // Not important enough to surface — the restore flow works without it.
  }
}

async function onFile(event) {
  const chosen = event.target.files?.[0]
  if (!chosen) return
  file.value = chosen
  report.value = null
  result.value = null
  typed.value = ''
  error.value = ''
  inspecting.value = true
  try {
    const res = await uploadBackupForRestore(chosen)
    report.value = res.report
  } catch (e) {
    error.value = e.message
    file.value = null
  } finally {
    inspecting.value = false
  }
}

async function doRestore() {
  if (!canRestore.value) return
  restoring.value = true
  error.value = ''
  try {
    result.value = await uploadBackupForRestore(file.value, CONFIRM_PHRASE)
    report.value = null
    file.value = null
    typed.value = ''
    // The API restarts itself afterwards to rebuild its connection pool, so give
    // it a moment before asking it anything again.
    setTimeout(loadSafetyCopies, 6000)
  } catch (e) {
    error.value = e.message
  } finally {
    restoring.value = false
  }
}

function formatBytes(n) {
  if (n == null) return '—'
  if (n >= 1048576) return `${(n / 1048576).toFixed(1)} MB`
  if (n >= 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${n} bytes`
}
</script>

<template>
  <div class="card">
    <h2>Restore from a backup</h2>

    <p class="error" v-if="error">{{ error }}</p>

    <p class="muted">
      Upload a <code>.sql.gz.age</code> backup to replace the database with it.
      The file is decrypted with your backup passphrase and test-loaded into a
      scratch database first, so you can see exactly what is in it before
      anything live is touched. A file that will not decrypt is refused — which
      is also what stops this being a way to run arbitrary SQL on the server.
    </p>

    <!-- Result of a completed restore -->
    <div class="done" v-if="result">
      <p>
        <span class="badge ok-badge">Restored</span>
        {{ result.rows_before }} rows replaced with {{ result.rows_after }}.
      </p>
      <p class="muted" v-if="result.safety_copy">
        A copy of the previous database was saved as
        <code>{{ result.safety_copy }}</code> before the restore. It is listed
        below if you need to undo this.
      </p>
      <p class="muted">{{ result.note }}</p>
    </div>

    <div class="field">
      <label>Backup file</label>
      <input type="file" accept=".age,.sql.gz.age" @change="onFile" :disabled="inspecting || restoring" />
    </div>
    <p class="muted" v-if="inspecting">Decrypting and test-loading the file…</p>

    <!-- What is in the uploaded file -->
    <template v-if="report">
      <h3 class="sub-heading">What this file contains</h3>
      <dl class="summary">
        <dt>Tables</dt>
        <dd>{{ report.tables }}</dd>
        <dt>Rows</dt>
        <dd>{{ report.rows }}</dd>
        <dt>Size</dt>
        <dd>{{ formatBytes(report.sql_bytes) }} of SQL</dd>
        <dt v-if="report.source_version">From Postgres</dt>
        <dd v-if="report.source_version">{{ report.source_version }}</dd>
      </dl>

      <details class="tables">
        <summary>Per-table row counts</summary>
        <table class="count-table">
          <tbody>
            <tr v-for="c in report.counts" :key="c.table">
              <td>{{ c.table }}</td>
              <td class="num">{{ c.rows }}</td>
            </tr>
          </tbody>
        </table>
      </details>

      <p class="warn">
        <strong>This replaces everything.</strong> Every customer, order, blend
        and audit entry currently in the database is discarded and replaced with
        the contents above. It cannot be undone from here — though a copy of the
        current database is saved first, and listed below.
      </p>

      <div class="field">
        <label>Type <code>{{ CONFIRM_PHRASE }}</code> to confirm</label>
        <input v-model="typed" type="text" autocomplete="off" spellcheck="false" />
      </div>

      <button class="danger-button" type="button" :disabled="!canRestore" @click="doRestore">
        {{ restoring ? 'Restoring…' : 'Replace the database with this backup' }}
      </button>
    </template>

    <!-- The way back -->
    <template v-if="safetyCopies.length">
      <h3 class="sub-heading">Safety copies</h3>
      <p class="muted">
        Taken automatically just before each restore, encrypted with the same
        passphrase. Download one and upload it above to undo a restore.
      </p>
      <ul class="copies">
        <li v-for="c in safetyCopies" :key="c.name">
          <a :href="`/api/admin/backup/safety-copies/${c.name}`" download>{{ c.name }}</a>
          <span class="muted"> — {{ formatBytes(c.bytes) }}</span>
        </li>
      </ul>
    </template>
  </div>
</template>

<style scoped>
.sub-heading {
  margin: 1.4rem 0 0.5rem;
  font-size: 0.95rem;
}

/* This is the one control in the app that discards everything. It should not
   look like the others. */
.danger-button {
  padding: 0.7rem 1.2rem;
  min-height: var(--tap);
  border: 1px solid var(--danger);
  border-radius: var(--radius);
  background: var(--danger);
  color: #fff;
  font: inherit;
  font-size: 0.82rem;
  font-weight: 700;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  cursor: pointer;
}

.danger-button:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.warn {
  margin: 1rem 0;
  padding: 0.8rem 1rem;
  background: color-mix(in srgb, var(--danger) 10%, var(--surface));
  border: 1px solid var(--danger);
  border-radius: var(--radius);
  font-size: 0.9rem;
  line-height: 1.5;
}

.done {
  margin: 0 0 1rem;
  padding: 0.8rem 1rem;
  background: var(--surface-alt);
  border-radius: var(--radius);
}

.done p {
  margin: 0 0 0.4rem;
}

.tables {
  margin: 0.6rem 0 0;
  font-size: 0.85rem;
}

.count-table {
  width: 100%;
  max-width: 26rem;
  border-collapse: collapse;
  margin-top: 0.5rem;
}

.count-table td {
  padding: 0.25rem 0.6rem 0.25rem 0;
  border-bottom: 1px solid var(--border);
}

.count-table .num {
  text-align: right;
  font-variant-numeric: tabular-nums;
}

.copies {
  margin: 0.5rem 0 0;
  padding-left: 1.1rem;
  font-size: 0.85rem;
}

.copies li {
  margin-bottom: 0.25rem;
}
</style>
