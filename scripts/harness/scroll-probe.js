/* Opt-in scroll hitch probe. Load the harness with ?probe=scroll.
 *
 * Drives the message list the way a flick does — down, then up twice —
 * and reports mounted row count plus requestAnimationFrame gaps. Subjects
 * and addresses stay out of the log. */
(function () {
  if (!/[?&]probe=scroll(?:&|$)/.test(String(location.search))) return;

  function log(obj) {
    try {
      console.info('[scroll-probe]', JSON.stringify(obj));
    } catch (e) {}
  }

  function run() {
    var scroller = document.querySelector('.scroller');
    var n = document.querySelectorAll('.row').length;
    if (!scroller || n === 0) {
      setTimeout(run, 120);
      return;
    }

    log({
      kind: 'start',
      rows: n,
      scrollHeight: scroller.scrollHeight,
      clientHeight: scroller.clientHeight,
    });

    var last = null;
    var frames = 0;
    var over50 = 0;
    var maxGap = 0;
    var dir = 1;
    var cycles = 0;
    var maxRows = n;

    function finish() {
      var payload = {
        kind: 'done',
        rows: document.querySelectorAll('.row').length,
        maxRows: maxRows,
        frames: frames,
        maxGap: Math.round(maxGap),
        over50: over50,
      };
      log(payload);
      window.__SCROLL_PROBE_DONE = payload;
    }

    function step(now) {
      if (last != null) {
        var gap = now - last;
        frames += 1;
        if (gap > maxGap) maxGap = gap;
        if (gap > 50) over50 += 1;
      }
      last = now;

      var mounted = document.querySelectorAll('.row').length;
      if (mounted > maxRows) maxRows = mounted;

      scroller.scrollTop += dir * 96;
      var atEnd = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 1;
      var atTop = scroller.scrollTop <= 0;
      if (dir === 1 && atEnd) dir = -1;
      else if (dir === -1 && atTop) {
        cycles += 1;
        if (cycles >= 2) {
          finish();
          return;
        }
        dir = 1;
      }
      requestAnimationFrame(step);
    }

    requestAnimationFrame(step);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', run);
  } else {
    run();
  }
})();
