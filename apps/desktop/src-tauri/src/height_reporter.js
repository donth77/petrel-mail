(function () {
  // Below this, shrinking stops being a way to read the message and becomes a
  // way to be unable to. What is left over scrolls sideways instead.
  var MIN_SCALE = 0.5;
  var fitting = false;
  var lastTransform = null;
  var lastBoxHeight = null;
  var lastPosted = null;
  var raf = 0;

  // Fits a too-wide message by scaling it, rather than cutting it off.
  //
  // Mail is full of layouts built to a fixed width, and a reading pane is
  // whatever width the window happens to be. Three things can happen to the
  // difference: the content is clipped, which loses it with no way to reach it;
  // it is squeezed, which takes fixed-width designs apart cell by cell; or the
  // whole thing is scaled down as one piece, which is the only one of the three
  // that keeps the message looking like itself.
  //
  // Scaling, not resizing: a transform leaves the layout alone, so nothing
  // reflows and the proportions the sender chose survive intact. The cost is
  // that a transform does not change the space the element reserves, so the
  // box around it has to be told the scaled height or the frame keeps a band of
  // blank space under short, wide mail.
  function fit() {
    var box = document.getElementById('petrel-box');
    var inner = document.getElementById('petrel-fit');
    if (!box || !inner || fitting) return;
    fitting = true;
    try {
      // Measured unscaled: clientWidth/scrollWidth ignore transform, so we
      // never clear transform/height to measure — that mutation retriggers
      // ResizeObserver and loops.
      var avail = inner.clientWidth;
      var natural = inner.scrollWidth;
      var transform = '';
      if (avail > 0 && natural > avail + 1) {
        var k = Math.max(avail / natural, MIN_SCALE);
        transform = 'scale(' + k + ')';
      }
      if (transform !== lastTransform) {
        inner.style.transform = transform;
        lastTransform = transform;
      }
      if (transform) {
        var h = Math.ceil(inner.getBoundingClientRect().height) + 'px';
        if (h !== lastBoxHeight) {
          box.style.height = h;
          lastBoxHeight = h;
        }
      } else if (lastBoxHeight !== '') {
        box.style.height = '';
        lastBoxHeight = '';
      }
    } catch (e) {}
    fitting = false;
  }

  // The host sizes the iframe to this number. scrollHeight still includes
  // the unscaled overflow of a transformed message, which is how a short
  // wide mail left a blank band under the text. The box already has the
  // height the fitter decided; its bottom plus the padding under it is
  // the space the message actually occupies.
  function h() {
    var box = document.getElementById('petrel-box');
    var b = document.body;
    if (box && b) {
      var pad = parseFloat(getComputedStyle(b).paddingBottom) || 0;
      return Math.ceil(box.getBoundingClientRect().bottom + pad);
    }
    var d = document.documentElement;
    return Math.max(d.scrollHeight, b ? b.scrollHeight : 0);
  }

  function post() {
    if (raf) return;
    raf = requestAnimationFrame(function () {
      raf = 0;
      fit();
      var height = h();
      var blocked = typeof BLOCKED !== 'undefined' ? BLOCKED : 0;
      var payload = height + ':' + blocked;
      if (payload === lastPosted) return;
      lastPosted = payload;
      try {
        parent.postMessage({ petrelHeight: height, petrelBlocked: blocked }, '*');
      } catch (e) {}
    });
  }

  addEventListener('load', post);
  addEventListener('resize', post);
  var observeTarget = document.getElementById('petrel-fit') || document.getElementById('petrel-box');
  if (window.ResizeObserver && observeTarget) {
    new ResizeObserver(post).observe(observeTarget);
  }
  post();
  setTimeout(post, 60);
  setTimeout(post, 400);

  // Where a link actually goes, reported out for the app to show.
  //
  // This is a security control as much as a convenience. Phishing *is* link
  // text that disagrees with its destination, and mail is where it lands — so
  // the one habit worth supporting is looking before clicking. A browser gives
  // you that for free in its status bar; a reading pane has to be told to.
  // It matters more here than in a browser, because the link opens somewhere
  // else entirely and there is no address bar to check on the way.
  function hover(url) {
    try { parent.postMessage({ petrelHover: url || '' }, '*'); } catch (e) {}
  }
  addEventListener('mouseover', function (e) {
    var a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (a) hover(a.href);
  });
  addEventListener('mouseout', function (e) {
    var a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (a) hover('');
  });
  // A link can be left by scrolling or by the pointer leaving the frame
  // altogether, neither of which fires mouseout on the anchor.
  addEventListener('blur', function () { hover(''); });
  document.addEventListener('mouseleave', function () { hover(''); });

  // Links leave the frame, they do not navigate it.
  //
  // Left alone, a click would replace the message with whatever the sender
  // linked to — a live web page loaded inside the reading pane, no longer
  // carrying this response's CSP. So every click is caught here and the
  // destination handed out to the app, which decides what opening it means.
  // The frame never navigates and never opens anything itself.
  addEventListener('click', function (e) {
    var a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (!a) return;
    e.preventDefault();
    try { parent.postMessage({ petrelOpen: a.href }, '*'); } catch (err) {}
  });

  // The reading-size preference. A CSS variable on the host cannot cross into
  // an opaque-origin frame, so the size is sent in and applied here — which is
  // also why it takes effect immediately rather than on the next fetch.
  addEventListener('message', function (e) {
    var n = e.data && e.data.petrelSize;
    // Bounded: the only thing this accepts is a plausible font size.
    if (typeof n === 'number' && n >= 10 && n <= 28) {
      document.documentElement.style.setProperty('--petrel-size', n + 'px');
      post();
    }
  });

  // Find in this message.
  //
  // Here rather than in the app because nothing outside can read this document:
  // the frame is opaque-origin by design, so the host cannot walk its text, and
  // window.find would search the app's own chrome instead. The app sends a term
  // and gets back a count; stepping between matches is the app's job, because
  // only it knows about the other messages in the thread.
  var found = [];

  function clearFind() {
    for (var i = 0; i < found.length; i++) {
      var m = found[i];
      var parent = m.parentNode;
      if (!parent) continue;
      parent.replaceChild(document.createTextNode(m.textContent), m);
      parent.normalize();
    }
    found = [];
  }

  function runFind(term) {
    clearFind();
    if (!term) { post(); return; }
    var needle = term.toLowerCase();
    // Text nodes only, and never inside a mark we just made — otherwise the
    // walk finds its own highlights and recurses.
    var walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, {
      acceptNode: function (n) {
        if (!n.nodeValue || !n.nodeValue.trim()) return NodeFilter.FILTER_REJECT;
        var p = n.parentNode;
        while (p && p !== document.body) {
          var tag = p.nodeName;
          if (tag === 'SCRIPT' || tag === 'STYLE') return NodeFilter.FILTER_REJECT;
          p = p.parentNode;
        }
        return n.nodeValue.toLowerCase().indexOf(needle) >= 0
          ? NodeFilter.FILTER_ACCEPT
          : NodeFilter.FILTER_REJECT;
      },
    });
    var targets = [];
    var node;
    while ((node = walker.nextNode())) targets.push(node);

    for (var t = 0; t < targets.length; t++) {
      var text = targets[t].nodeValue;
      var lower = text.toLowerCase();
      var frag = document.createDocumentFragment();
      var at = 0;
      var hit;
      while ((hit = lower.indexOf(needle, at)) >= 0) {
        if (hit > at) frag.appendChild(document.createTextNode(text.slice(at, hit)));
        var mark = document.createElement('mark');
        mark.className = 'petrel-find';
        mark.textContent = text.slice(hit, hit + needle.length);
        frag.appendChild(mark);
        found.push(mark);
        at = hit + needle.length;
      }
      if (at < text.length) frag.appendChild(document.createTextNode(text.slice(at)));
      targets[t].parentNode.replaceChild(frag, targets[t]);
    }
    try { parent.postMessage({ petrelFound: found.length }, '*'); } catch (e) {}
    post();
  }

  function setActive(i) {
    for (var n = 0; n < found.length; n++) {
      found[n].className = n === i ? 'petrel-find on' : 'petrel-find';
    }
    if (found[i] && found[i].scrollIntoView) {
      found[i].scrollIntoView({ block: 'center' });
    }
  }

  addEventListener('message', function (e) {
    var d = e.data || {};
    if (typeof d.petrelFind === 'string') runFind(d.petrelFind);
    if (typeof d.petrelFindActive === 'number') setActive(d.petrelFindActive);
  });

  addEventListener('keydown', function (e) {
    // Identity only — which key, which modifiers. Nothing about the document.
    try {
      parent.postMessage({
        petrelKey: {
          key: e.key,
          metaKey: e.metaKey, ctrlKey: e.ctrlKey,
          shiftKey: e.shiftKey, altKey: e.altKey
        }
      }, '*');
    } catch (err) {}
  });
})();
