/* Thanks page. Square returns shop orders here as /thanks?from=shop; those
   are shipped rather than collected, and the bag they came from is now paid
   for and should be emptied. Without this script the page still reads
   correctly for a share-link purchase, which is the default copy. */
(function () {
  'use strict'
  if (new URLSearchParams(location.search).get('from') !== 'shop') return

  try {
    localStorage.removeItem('blendbar.bag.v1')
  } catch {
    /* Storage blocked: nothing was saved, so there is nothing to clear. */
  }

  const detail = document.getElementById('thanksDetail')
  if (detail) {
    detail.textContent =
      'We’ll pack your order and ship it to the address you gave at checkout. ' +
      'Anything we blend by hand is made to order, so allow a few extra days for those.'
  }
  const action = document.getElementById('thanksAction')
  if (action) {
    action.textContent = 'Back to the shop'
    action.setAttribute('href', '/shop')
  }
})()
