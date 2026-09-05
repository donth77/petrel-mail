/* Opt-in height-reporter loop probe. Load the harness with ?probe=fit.
 *
 * Mounts the wide stand-in frame and counts petrelHeight posts for two
 * seconds. A loop writes every frame; a settled fitter posts a handful
 * of times (load, 60ms, 400ms, maybe one resize). Subjects stay out.
 *
 * The posted height must be the fitted box, not document.scrollHeight.
 * scrollHeight still includes the unscaled overflow, which is the blank
 * band under a short wide message. */
(function () {
  if (!/[?&]probe=fit(?:&|$)/.test(String(location.search))) return;

  function log(obj) {
    try {
      console.info('[fit-probe]', JSON.stringify(obj));
    } catch (e) {}
  }

  function occupiedHeight(doc) {
    var box = doc.getElementById('petrel-box');
    var body = doc.body;
    if (!box || !body) return null;
    var pad = parseFloat(doc.defaultView.getComputedStyle(body).paddingBottom) || 0;
    return Math.ceil(box.getBoundingClientRect().bottom + pad);
  }

  function run() {
    var posts = 0;
    var lastHeight = null;
    window.addEventListener('message', function (e) {
      if (e.data && typeof e.data.petrelHeight === 'number') {
        posts += 1;
        lastHeight = e.data.petrelHeight;
      }
    });

    var frame = document.createElement('iframe');
    frame.className = 'msg-frame';
    // Same-origin so the probe can read the box it is checking against.
    // The production frame does not grant this.
    frame.setAttribute('sandbox', 'allow-scripts allow-same-origin');
    frame.src = './msg.html?wide=1';
    frame.style.width = '320px';
    frame.style.height = '180px';
    document.body.appendChild(frame);

    setTimeout(function () {
      var doc = frame.contentDocument;
      var occupied = doc ? occupiedHeight(doc) : null;
      var scroll = doc && doc.body
        ? Math.max(doc.documentElement.scrollHeight, doc.body.scrollHeight)
        : null;
      var heightOk = occupied != null && lastHeight === occupied;
      var payload = {
        kind: 'done',
        posts: posts,
        height: lastHeight,
        occupied: occupied,
        scrollHeight: scroll,
        pass: posts <= 20 && heightOk,
      };
      log(payload);
      window.__FIT_PROBE_DONE = payload;
    }, 2000);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', run);
  } else {
    run();
  }
})();
