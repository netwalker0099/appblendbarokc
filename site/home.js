/* Home page enhancements: the scroll reveal and the footer year.
   Served under a strict CSP (script-src 'self'), so this is an external file
   with no inline handlers anywhere.

   The reveal is deliberately additive. `.reveal` in the stylesheet does nothing
   on its own; this script adds `.reveal-ready` (which hides an element) only to
   elements that are *below the fold at load*, then releases each one as it comes
   into view. Two consequences, both wanted:

     * With JS off, blocked, or broken, every section is simply visible. A
       decorative animation must never be able to hide the page.
     * Nothing above the fold is ever hidden, so there is no flash of the hero
       disappearing and coming back while the script boots. */
(function () {
  'use strict'

  var year = document.getElementById('year')
  if (year) year.textContent = String(new Date().getFullYear())

  var targets = Array.prototype.slice.call(document.querySelectorAll('.reveal'))
  if (!targets.length) return

  // No IntersectionObserver (or the visitor asked for less motion): leave
  // everything on screen exactly as the HTML shipped it.
  var reduced =
    window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches
  if (reduced || !('IntersectionObserver' in window)) return

  // A little past the fold, so an element that is only just below the crease
  // doesn't animate the instant the page settles.
  var fold = window.innerHeight * 0.92

  var pending = targets.filter(function (el) {
    return el.getBoundingClientRect().top > fold
  })
  pending.forEach(function (el) {
    el.classList.add('reveal-ready')
  })
  if (!pending.length) return

  var observer = new IntersectionObserver(
    function (entries) {
      entries.forEach(function (entry) {
        if (!entry.isIntersecting) return
        entry.target.classList.add('in')
        // One reveal per element; releasing it again on the way back up would
        // make the page twitch while scrolling.
        observer.unobserve(entry.target)
      })
    },
    // Fire a little before the element's top edge arrives, so the movement has
    // finished by the time it is properly in view.
    { rootMargin: '0px 0px -12% 0px', threshold: 0.05 },
  )

  pending.forEach(function (el) {
    observer.observe(el)
  })
})()
