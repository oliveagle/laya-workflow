// dismiss.js — close a full-viewport modal that would swallow our clicks.
//
// Sites pop a promo/login modal whose mask covers the whole viewport with
// pointer-events:auto, so every later "real" click lands on the mask and does
// nothing. This finds each full-viewport overlay, clicks its close control, and
// reports whether something still covers the centre of the screen.
//
// Two shapes need care. The Alibaba anti-bot shell (`.baxia-dialog`) is a *fixed,
// full-viewport* dialog whose close control is a *sibling* of the mask rather than
// a child, so a mask-first search never reaches it — and closing it is exactly the
// "亲，请拖动下方滑块完成验证" box that otherwise blocks the page. It also has no
// text of its own (the challenge is in a child iframe), so nothing but the class
// name identifies it.
(function () {
  "use strict";
  var vw = window.innerWidth, vh = window.innerHeight;

  function covers(e) {
    if (!e || !e.getBoundingClientRect) return false;
    var r = e.getBoundingClientRect();
    if (r.width < vw * 0.85 || r.height < vh * 0.85) return false;
    var s = getComputedStyle(e);
    return s.display !== "none" && s.visibility !== "hidden"
      && s.pointerEvents !== "none" && s.position !== "static";
  }

  var overlays = [];
  var all = document.querySelectorAll("div,section,aside,dialog,main");
  for (var i = 0; i < all.length; i++) if (covers(all[i])) overlays.push(all[i]);

  var CTL = '[class*="baxia-dialog-close"],[class*="close" i],[class*="cross" i]'
    + ',[aria-label*="关闭"],button';
  function controlIn(root) {
    try { return root && root.querySelector ? root.querySelector(CTL) : null; } catch (e) { return null; }
  }
  function namedClose() {
    // A close button identified by its own class name - not "the first button on
    // the page", which is what makes this safe to try outside the overlay.
    try { return document.querySelector('[class*="baxia-dialog-close"]'); } catch (e) { return null; }
  }

  var closed = [];
  for (var j = 0; j < overlays.length; j++) {
    var ov = overlays[j];
    var ctl = controlIn(ov)
      || (covers(ov.parentElement) ? controlIn(ov.parentElement) : null);
    if (ctl) {
      try {
        ctl.click();
        closed.push(String(ctl.className || ctl.tagName).trim().slice(0, 40));
      } catch (e) {}
    }
  }
  // The close control may live outside any overlay we found (the mask alone can be
  // the only full-viewport node); name that one explicitly.
  if (!closed.length) {
    var bax = namedClose();
    if (bax) { try { bax.click(); closed.push("baxia-dialog-close"); } catch (e) {} }
  }

  // What is still on top of the middle of the screen?
  var blocking = null;
  var hit = document.elementFromPoint(vw / 2, vh / 2);
  for (var e = hit; e && e !== document.body; e = e.parentElement) {
    if (covers(e)) { blocking = String(e.className || e.tagName).slice(0, 60); break; }
  }
  return { overlays: overlays.length, closed: closed.slice(0, 8), still_blocking: blocking };
})()
