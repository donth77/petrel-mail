/* Opt-in height-reporter loop probe. Load the harness with ?probe=fit.
 *
 * Mounts the wide stand-in frame and counts petrelHeight posts for two
 * seconds. A loop writes every frame; a settled fitter posts a handful
 * of times (load, 60ms, 400ms, maybe one resize). Subjects stay out. */
(function () {
  if (!/[?&]probe=fit(?:&|$)/.test(String(location.search))) return;

  function log(obj) {
    try {
      console.info('[fit-probe]', JSON.stringify(obj));
    } catch (e) {}
  }

  function run() {
    var posts = 0;
    window.addEventListener('message', function (e) {
      if (e.data && typeof e.data.petrelHeight === 'number') posts += 1;
    });

    var frame = document.createElement('iframe');
    frame.className = 'msg-frame';
    frame.setAttribute('sandbox', 'allow-scripts');
    frame.src = './msg.html?wide=1';
    frame.style.width = '320px';
    frame.style.height = '180px';
    document.body.appendChild(frame);

    setTimeout(function () {
      var payload = { kind: 'done', posts: posts, pass: posts <= 20 };
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
