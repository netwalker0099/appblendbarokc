<script setup>
/**
 * Event booking enquiries from the public form at /book.
 *
 * This is the record of record. The chat notification is a prompt to come and
 * look at this list — it deliberately carries only what is needed to decide
 * whether to drop what you are doing, and (unless a channel opts in) leaves the
 * contact details here rather than mirroring them into a third party.
 *
 * The list is a worklist, not a CRM: four states, an internal note, and who
 * took it. The filter defaults to what still needs a human, because an enquiry
 * nobody answers is the failure this whole feature exists to prevent.
 */
import { computed, onMounted, ref, watch } from 'vue'

import { api } from '../lib/api.js'

const emit = defineEmits(['changed'])

const FILTERS = [
  { value: 'open', label: 'Needs a reply' },
  { value: 'all', label: 'All' },
  { value: 'booked', label: 'Booked' },
  { value: 'declined', label: 'Declined' },
]

const STATUSES = [
  { value: 'new', label: 'New' },
  { value: 'contacted', label: 'Contacted' },
  { value: 'booked', label: 'Booked' },
  { value: 'declined', label: 'Declined' },
]

const filter = ref('open')
const enquiries = ref([])
const openCount = ref(0)
const loading = ref(false)
const busy = ref('')
const error = ref('')
const notice = ref('')

/** Which row has its note editor open, and the text being edited. */
const editing = ref('')
const noteDraft = ref('')

async function load() {
  loading.value = true
  error.value = ''
  try {
    const res = await api.listEnquiries(filter.value)
    enquiries.value = res.enquiries || []
    openCount.value = res.open_count ?? 0
    emit('changed', openCount.value)
  } catch (err) {
    error.value = err.message
  } finally {
    loading.value = false
  }
}

watch(filter, load)
onMounted(load)

async function setStatus(row, status) {
  busy.value = row.id
  error.value = ''
  notice.value = ''
  try {
    await api.updateEnquiry(row.id, { status })
    await load()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

function startNote(row) {
  editing.value = row.id
  noteDraft.value = row.staff_notes || ''
}

async function saveNote(row) {
  busy.value = row.id
  error.value = ''
  try {
    await api.updateEnquiry(row.id, { staff_notes: noteDraft.value })
    editing.value = ''
    notice.value = 'Note saved.'
    await load()
  } catch (err) {
    error.value = err.message
  } finally {
    busy.value = ''
  }
}

/** The event date as written — a plain date, with no timezone to shift it. */
function formatEventDate(value) {
  if (!value) return '—'
  const [y, m, d] = value.split('-').map(Number)
  return new Date(y, m - 1, d).toLocaleDateString(undefined, {
    weekday: 'short',
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  })
}

function formatTime(value) {
  return value ? new Date(value).toLocaleString() : '—'
}

/** Days until the event; negative once it has passed. */
function daysAway(value) {
  if (!value) return null
  const [y, m, d] = value.split('-').map(Number)
  const then = new Date(y, m - 1, d)
  const now = new Date()
  now.setHours(0, 0, 0, 0)
  return Math.round((then - now) / 86400000)
}

/** Urgency, so a date two weeks out is visible without reading every row. */
function dateNote(row) {
  const n = daysAway(row.event_date)
  if (n === null) return ''
  if (n < 0) return `${Math.abs(n)} days ago`
  if (n === 0) return 'today'
  if (n === 1) return 'tomorrow'
  return `in ${n} days`
}

function isUrgent(row) {
  const n = daysAway(row.event_date)
  return n !== null && n >= 0 && n <= 21 && (row.status === 'new' || row.status === 'contacted')
}

const heading = computed(() =>
  openCount.value ? `Event enquiries (${openCount.value} open)` : 'Event enquiries',
)
</script>

<template>
  <div class="card">
    <h2>{{ heading }}</h2>
    <p class="muted">
      Sent from the booking form at <code>/book</code> on the customer site. The
      full details are here; a chat channel only gets the contact details if that
      channel is set to include them.
    </p>

    <p class="error" v-if="error">{{ error }}</p>
    <p class="notice" v-if="notice">{{ notice }}</p>

    <div class="row filter-row" style="flex-wrap: wrap; gap: 0.5rem; margin-bottom: 0.9rem">
      <button
        v-for="f in FILTERS"
        :key="f.value"
        class="ghost"
        type="button"
        :class="{ active: filter === f.value }"
        @click="filter = f.value"
      >
        {{ f.label }}
      </button>
      <button class="ghost" type="button" :disabled="loading" @click="load()">
        {{ loading ? 'Refreshing…' : 'Refresh' }}
      </button>
    </div>

    <p class="muted" v-if="!loading && !enquiries.length">
      {{ filter === 'open' ? 'Nothing waiting on a reply.' : 'No enquiries yet.' }}
    </p>

    <div v-for="row in enquiries" :key="row.id" class="enquiry">
      <div class="enquiry-head">
        <div class="grow">
          <strong>{{ row.first_name }} {{ row.last_name }}</strong>
          <span class="muted">
            {{ formatEventDate(row.event_date) }}
            <span :class="{ 'danger-text': isUrgent(row) }">· {{ dateNote(row) }}</span>
          </span>
          <span class="muted">Received {{ formatTime(row.created_at) }}</span>
        </div>
        <span class="badge" :class="row.status === 'booked' ? 'ok-badge' : ''">
          {{ STATUSES.find((s) => s.value === row.status)?.label || row.status }}
        </span>
      </div>

      <p class="enquiry-details">{{ row.details }}</p>

      <div class="row enquiry-contact">
        <a :href="`mailto:${row.email}`">{{ row.email }}</a>
        <a :href="`tel:${row.phone.replace(/[^+\d]/g, '')}`">{{ row.phone }}</a>
      </div>

      <p class="muted enquiry-note" v-if="row.staff_notes && editing !== row.id">
        <strong>Note:</strong> {{ row.staff_notes }}
      </p>

      <div v-if="editing === row.id" style="margin: 0.6rem 0">
        <label>Internal note</label>
        <textarea
          v-model="noteDraft"
          rows="3"
          placeholder="Anything the team should know. Never shown to the customer."
        ></textarea>
        <div class="row" style="margin-top: 0.4rem">
          <button class="ghost" type="button" :disabled="busy === row.id" @click="saveNote(row)">
            Save note
          </button>
          <button class="ghost" type="button" @click="editing = ''">Cancel</button>
        </div>
      </div>

      <div class="row enquiry-actions">
        <button
          v-for="s in STATUSES"
          :key="s.value"
          class="ghost"
          type="button"
          :disabled="busy === row.id || row.status === s.value"
          @click="setStatus(row, s.value)"
        >
          {{ s.label }}
        </button>
        <button class="ghost" type="button" v-if="editing !== row.id" @click="startNote(row)">
          {{ row.staff_notes ? 'Edit note' : 'Add note' }}
        </button>
      </div>

      <p class="muted" v-if="row.handled_at" style="font-size: 0.8rem">
        Last moved {{ formatTime(row.handled_at) }}
      </p>
    </div>
  </div>
</template>

<style scoped>
.enquiry {
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 0.9rem 1rem;
  margin-bottom: 0.8rem;
}

.enquiry-head {
  display: flex;
  align-items: flex-start;
  gap: 1rem;
}

.enquiry-head .grow {
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  flex: 1;
}

/* The customer's own words. `pre-wrap` because they typed line breaks and a
   paragraph that ignores them is harder to read than one that does not. */
.enquiry-details {
  white-space: pre-wrap;
  margin: 0.7rem 0;
}

.enquiry-contact {
  gap: 1rem;
  font-size: 0.9rem;
}

.enquiry-note {
  margin: 0.5rem 0 0;
}

.enquiry-actions {
  flex-wrap: wrap;
  gap: 0.4rem;
  margin-top: 0.7rem;
}

.ghost.active {
  border-color: var(--accent);
  color: var(--accent);
}

/* "Needs a reply" is longer than the other filters and wrapped to two lines,
   leaving the row uneven. */
.filter-row .ghost {
  white-space: nowrap;
}
</style>
