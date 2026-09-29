// dismiss.js — close a full-viewport modal that would swallow our clicks.
//
// Sites pop a promo/login modal whose mask covers the whole viewport with
// pointer-events:auto, so every later "real" click lands on the mask and does
// nothing. This finds each full-viewport overlay, clicks its close control
// (a `class*=close/cross`, an aria-label with 关闭, or a plain button), and
// reports whether something still covers the centre of the screen.
(function () {
  "use strict";
  var vw = window.innerWidth, vh = window.innerHeight;

  function covers(e) {
    var r = e.getBoundingClientRect();
    if (r.width < vw * 0.85 || r.height < vh * 0.85) return false;
    var s = getComputedStyle(e);
    return s.display !== "none" && s.visibility !== "hidden"
      && s.pointerEvents !== "none" && s.position !== "static";
  }

  var overlays = [];
  var all = document.querySelectorAll("div,section,aside,dialog,main");
  for (var i = 0; i < all.length; i++) if (covers(all[i])) overlays.push(all[i]);

  var closed = [];
  for (var j = 0; j < overlays.length; j++) {
    var ctl = overlays[j].querySelector(
      '[class*="close" i],[class*="cross" i],[aria-label*="关闭"],button'
    );
    if (ctl) {
      try { ctl.click(); closed.push(String(ctl.className || ctl.tagName).slice(0, 40)); } catch (e) {}
    }
  }

  // What is still on top of the middle of the screen?
  var blocking = null;
  var hit = document.elementFromPoint(vw / 2, vh / 2);
  for (var e = hit; e && e !== document.body; e = e.parentElement) {
    if (covers(e)) { blocking = String(e.className || e.tagName).slice(0, 60); break; }
  }
  return { overlays: overlays.length, closed: closed.slice(0, 8), still_blocking: blocking };
})()
