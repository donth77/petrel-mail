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

  // The host sizes the iframe to this number. A transformed message still
  // occupies its unscaled layout height, so scrollHeight of that document is
  // a blank band under the text — the box already has the fitted height.
  // Without a transform the tallest of the box, the fit element and its
  // scrollable extent is taken: a float or an oversized child can hang
  // below the box it sits in, and a rect alone cut those off.
  //
  // Past the cap the frame is allowed to scroll on its own, once: the host
  // used to drop any report this tall and leave the frame at first paint.
  var HEIGHT_CAP = 200000;

  function contentLayoutHeight(inner, pad) {
    var top = 0;
    var el = inner;
    while (el) {
      top += el.offsetTop;
      el = el.offsetParent;
    }
    return top + inner.scrollHeight + pad;
  }

  function h() {
    var box = document.getElementById('petrel-box');
    var inner = document.getElementById('petrel-fit');
    var b = document.body;
    var d = document.documentElement;
    var pad = b ? (parseFloat(getComputedStyle(b).paddingBottom) || 0) : 0;
    if (box && b && lastTransform) {
      return Math.ceil(box.getBoundingClientRect().bottom + pad);
    }
    var boxH = box && b ? box.getBoundingClientRect().bottom + pad : 0;
    var innerH = inner && b ? inner.getBoundingClientRect().bottom + pad : 0;
    var layout = inner && b ? contentLayoutHeight(inner, pad) : 0;
    var measured = Math.max(boxH, innerH, layout);
    if (measured > 0) return Math.ceil(measured);
    return Math.max(d.scrollHeight, b ? b.scrollHeight : 0);
  }

  // A message whose height follows the window — a viewport unit the
  // sanitizer did not catch, or something newer — grows by exactly what the
  // host just gave it, the host gives it that again, and the two chase each
  // other up to the cap. Three rounds of growth matching the last resize is
  // that chase and nothing else; the height is then held where it was.
  var CHASE_ROUNDS = 3;
  var lastInner = null;
  var lastHeight = null;
  var chasing = 0;
  var held = null;

  function holdIfChasingTheWindow(height) {
    if (held != null) return held;
    var inner = window.innerHeight;
    if (lastHeight != null && lastInner != null) {
      var grew = height - lastHeight;
      var given = inner - lastInner;
      if (given > 0) {
        chasing = grew >= given - 2 ? chasing + 1 : 0;
      } else if (grew !== 0) {
        // The content moved with no resize behind it: not a chase.
        chasing = 0;
      }
      // Neither moved: a re-measure the observer asked for, nothing to learn.
      if (chasing >= CHASE_ROUNDS) {
        held = lastHeight;
        return held;
      }
    }
    lastHeight = height;
    lastInner = inner;
    return height;
  }

  // The document itself changed — a new text size, find marks — so a held
  // height is stale. Measure afresh; a chase starts over from here.
  function releaseHold() {
    held = null;
    chasing = 0;
    lastHeight = null;
    lastInner = null;
  }

  function post() {
    if (raf) return;
    raf = requestAnimationFrame(function () {
      raf = 0;
      fit();
      var height = holdIfChasingTheWindow(h());
      if (height >= HEIGHT_CAP) {
        height = HEIGHT_CAP;
        document.documentElement.style.overflowY = 'auto';
      }
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
      releaseHold();
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
    releaseHold();
    clearFind();
    // Find has the text to itself while it is in use. A search's marks split
    // the text nodes it walks, so "vendor contracts" would not be found across
    // a marked "vendor" — and they come back the moment find is put away.
    if (term) clearSearch(); else markSearch();
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

  // The words of the search that found this message, marked where they are.
  //
  // Sent in for the same reason find is done in here: nothing outside can read
  // this document. Words only come *in* — nothing about what they matched goes
  // back out, not even a count. They are matched the way the index matched
  // them: whole words, a phrase in order with anything that is not a letter
  // or a digit between its words, the last word as the start of one. The app
  // has the same few lines (`hitsIn` in search-highlight.tsx) for everything
  // outside the frame, and the two have to agree.
  var searchTerms = [];
  var searchMarks = [];
  var WORD = /[\p{L}\p{N}]/u;
  var ACCENT = /\p{M}/u;

  function accents(low, from) {
    var to = from;
    while (to < low.length && ACCENT.test(low.charAt(to))) to++;
    return to;
  }

  function fold(s) {
    var out = '';
    for (var i = 0; i < s.length; i++) {
      var c = s.charAt(i);
      var f = c > '\x7f' ? c.normalize('NFD').replace(/[̀-ͯ]/g, '') : c;
      f = f.toLowerCase();
      out += f.length === 1 ? f : c;
    }
    return out;
  }

  function hitsIn(low) {
    function word(at) { return at >= 0 && at < low.length && WORD.test(low.charAt(at)); }
    var hits = [];
    for (var n = 0; n < searchTerms.length; n++) {
      var term = searchTerms[n];
      var first = term.t[0];
      for (var at = low.indexOf(first); at >= 0; at = low.indexOf(first, at + 1)) {
        if (!term.c && word(at - 1)) continue;
        var end = at + first.length;
        var whole = true;
        for (var k = 1; k < term.t.length; k++) {
          var gap = end;
          while (gap < low.length && !word(gap)) gap++;
          if (gap === end || low.substr(gap, term.t[k].length) !== term.t[k]) { whole = false; break; }
          end = gap + term.t[k].length;
        }
        if (!whole) continue;
        if (!term.c) {
          // An accent typed as its own character belongs to the letter before
          // it: neither the end of the word nor outside the mark.
          end = accents(low, end);
          if (term.p) { while (word(end)) end = accents(low, end + 1); }
          else if (word(end)) continue;
        }
        hits.push([at, end]);
      }
    }
    hits.sort(function (a, b) { return a[0] - b[0] || b[1] - a[1]; });
    var merged = [];
    for (var h = 0; h < hits.length; h++) {
      var before = merged[merged.length - 1];
      if (before && hits[h][0] <= before[1]) before[1] = Math.max(before[1], hits[h][1]);
      else merged.push(hits[h]);
    }
    return merged;
  }

  function clearSearch() {
    for (var i = 0; i < searchMarks.length; i++) {
      var m = searchMarks[i];
      var parent = m.parentNode;
      if (!parent) continue;
      parent.replaceChild(document.createTextNode(m.textContent), m);
      parent.normalize();
    }
    searchMarks = [];
  }

  function markSearch() {
    clearSearch();
    // Not while find is showing its own marks; runFind calls back when it ends.
    if (!searchTerms.length || found.length) return;
    var walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, {
      acceptNode: function (n) {
        if (!n.nodeValue || !n.nodeValue.trim()) return NodeFilter.FILTER_REJECT;
        var p = n.parentNode;
        while (p && p !== document.body) {
          var tag = p.nodeName;
          if (tag === 'SCRIPT' || tag === 'STYLE') return NodeFilter.FILTER_REJECT;
          p = p.parentNode;
        }
        return NodeFilter.FILTER_ACCEPT;
      },
    });
    var targets = [];
    var node;
    while ((node = walker.nextNode())) targets.push(node);
    for (var t = 0; t < targets.length; t++) {
      var text = targets[t].nodeValue;
      var hits = hitsIn(fold(text));
      if (!hits.length) continue;
      var frag = document.createDocumentFragment();
      var at = 0;
      for (var h = 0; h < hits.length; h++) {
        if (hits[h][0] > at) frag.appendChild(document.createTextNode(text.slice(at, hits[h][0])));
        var mark = document.createElement('mark');
        mark.className = 'petrel-hit';
        mark.textContent = text.slice(hits[h][0], hits[h][1]);
        frag.appendChild(mark);
        searchMarks.push(mark);
        at = hits[h][1];
      }
      if (at < text.length) frag.appendChild(document.createTextNode(text.slice(at)));
      targets[t].parentNode.replaceChild(frag, targets[t]);
    }
  }

  // Bounded, and nothing but strings and booleans gets past: a term is a
  // short list of short folded words.
  function termsFrom(sent) {
    var out = [];
    if (!Array.isArray(sent)) return out;
    for (var i = 0; i < sent.length && out.length < 32; i++) {
      var s = sent[i] || {};
      if (!Array.isArray(s.t) || !s.t.length || s.t.length > 32) continue;
      var tokens = [];
      for (var k = 0; k < s.t.length; k++) {
        if (typeof s.t[k] === 'string' && s.t[k] && s.t[k].length <= 256) tokens.push(s.t[k]);
      }
      if (tokens.length === s.t.length) out.push({ t: tokens, p: s.p === true, c: s.c === true });
    }
    return out;
  }

  addEventListener('message', function (e) {
    var d = e.data || {};
    if (typeof d.petrelFind === 'string') runFind(d.petrelFind);
    if (typeof d.petrelFindActive === 'number') setActive(d.petrelFindActive);
    if (d.petrelSearch !== undefined) {
      searchTerms = termsFrom(d.petrelSearch);
      releaseHold();
      markSearch();
      post();
    }
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
