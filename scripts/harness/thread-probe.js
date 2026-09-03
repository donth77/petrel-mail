/* Opt-in reading-pane bound probe. Load the harness with ?probe=thread.
 *
 * Opens the first row, clicks a collapsed card if one is mounted, and
 * records mounted .msg-frame count plus how many fat hydrates the shim
 * saw. A 500-message conversation must not mount more than three bodies. */
(function () {
  if (!/[?&]probe=thread(?:=|&|$)/.test(String(location.search))) return;

  function log(obj) {
    try {
      console.info('[thread-probe]', JSON.stringify(obj));
    } catch (e) {}
  }

  function finish() {
    var frames = document.querySelectorAll('.msg-frame').length;
    var rec = window.__THREAD_PROBE__ || {};
    var fat = rec.fatCalls || 0;
    var payload = {
      kind: 'done',
      frames: frames,
      index: rec.index || 0,
      fatCalls: fat,
      pass: frames <= 3 && rec.index > 0 && fat > 0 && fat <= 4,
    };
    log(payload);
    window.__THREAD_PROBE_DONE = payload;
  }

  function clickCollapsed() {
    var card = document.querySelector('.collapsed');
    if (card) card.click();
    setTimeout(finish, 400);
  }

  function openFirst() {
    var row = document.querySelector('.row');
    if (!row) {
      setTimeout(openFirst, 120);
      return;
    }
    row.click();
    setTimeout(clickCollapsed, 400);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', openFirst);
  } else {
    openFirst();
  }
})();
