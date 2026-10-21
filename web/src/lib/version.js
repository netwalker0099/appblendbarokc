/// Notices when a newer build has been deployed while this tab stayed open.
///
/// The stand tablet keeps the app open for days, and a single-page app keeps
/// running the code it loaded until someone reloads — so after a deploy the
/// tablet can quietly behave like the old version. (It did on 2026-10-21: the
/// per-size blend rules were live, but an open intake tab still demanded
/// 100 ml for every bottle.) Each build's entry script has a content hash in
/// its name, so comparing the one this tab loaded with the one the server now
/// serves is enough to tell.
///
/// It only offers a reload, never forces one: an operator may be halfway
/// through an intake, and reloading would throw that away.
import { ref } from 'vue'

const ENTRY = /\/assets\/index-[\w-]+\.js/
const CHECK_EVERY_MS = 5 * 60 * 1000

export const updateAvailable = ref(false)

function loadedEntry() {
  for (const script of document.scripts) {
    const match = script.src.match(ENTRY)
    if (match) return match[0]
  }
  return null // the dev server, which has no hashed entry
}

async function check() {
  if (updateAvailable.value) return
  const mine = loadedEntry()
  if (!mine) return
  try {
    const res = await fetch('/', { cache: 'no-store', credentials: 'same-origin' })
    if (!res.ok) return
    const live = (await res.text()).match(ENTRY)?.[0]
    if (live && live !== mine) updateAvailable.value = true
  } catch {
    // Offline or mid-deploy: try again next time.
  }
}

let started = false

export function watchForUpdates() {
  if (started) return
  started = true
  check()
  setInterval(check, CHECK_EVERY_MS)
  // A tablet waking up is exactly when a stale tab is most likely.
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') check()
  })
}
