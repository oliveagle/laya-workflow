// find.js — turn an *intent* into one numbered decision.
//
// Runs inside the owned tab, reads the laya-browser extension's numbered
// snapshot, picks the element whose label matches the intent, draws the
// numbered badges so a human can see the same numbers, and returns the Jev
// decision that acts on it:
//
//   { operation, element, name, role, goal_probability, stuck_probability, … }
//
// The workflow injects `window.__LAYA__ = { operation, roles, match, nth, text, note }`.
(function () {
  "use strict";

  var O = (typeof window.__LAYA__ === "object" && window.__LAYA__) || {};
  var lb = window.__layaBrowser;
  if (!lb || typeof lb.snapshot !== "function") {
    throw new Error("laya-browser extension is not loaded in this tab (no window.__layaBrowser)");
  }

  function list(v) {
    if (v === undefined || v === null || v === "") return [];
    return Object.prototype.toString.call(v) === "[object Array]" ? v : [v];
  }
  var operation = String(O.operation || "CLICK").toUpperCase();
  var roles = list(O.roles).concat(list(O.role)).map(function (r) { return String(r).toLowerCase(); });
  var needles = list(O.match).map(function (m) { return String(m).toLowerCase(); });
  var targetHref = list(O.href).map(function (h) { return String(h).toLowerCase(); });
  var nth = parseInt(O.nth, 10) || 0;            // 1-based when > 0
  var exact = O.exact === true || O.exact === "true";
  var selector = O.selector ? String(O.selector) : "";
  var text = (O.text === undefined || O.text === null) ? "" : String(O.text);

  // Clear any tag left by a previous selector-fallback pick, so this round's
  // selector can never resolve to the element we tagged last time.
  try {
    var tagged = document.querySelectorAll("[data-laya-pick]");
    for (var t = 0; t < tagged.length; t++) tagged[t].removeAttribute("data-laya-pick");
  } catch (e) {}

  var snap = lb.snapshot(0, true);               // numbers every visible candidate
  var rows = snap.elements || [];

  function hit(name) {
    if (!needles.length) return true;
    var n = String(name || "").toLowerCase();
    for (var i = 0; i < needles.length; i++) {
      // `exact` is how a pager is told apart from its twin: Taobao's header
      // arrow is named "下一页，当前第1页" while the real pager is "下一页".
      if (exact ? n === needles[i] : n.indexOf(needles[i]) >= 0) return true;
    }
    return false;
  }

  function hrefHit(r) {
    if (!targetHref.length) return true;
    var h = String(r.href || "").toLowerCase();
    for (var i = 0; i < targetHref.length; i++) if (h.indexOf(targetHref[i]) >= 0) return true;
    return false;
  }

  var pool = rows.filter(function (r) {
    if (r.disabled) return false;
    if (roles.length && roles.indexOf(String(r.role).toLowerCase()) < 0) return false;
    if (!hrefHit(r)) return false;
    return hit(r.name);
  });
  var nearby = [];
  for (var i = 0; i < rows.length && nearby.length < 14; i++) {
    var r = rows[i];
    if (roles.length && roles.indexOf(String(r.role).toLowerCase()) < 0) continue;
    nearby.push("#" + r.index + " " + r.role + " " + String(r.name || "").slice(0, 24));
  }

  // Rank the matches by "actually clickable right now": scroll each into view and
  // require its centre to sit inside the viewport *and* to hit the element (or a
  // child) on top. The numbered snapshot also lists off-screen and shadowed
  // elements - e.g. Taobao numbers a tiny pager arrow that lives beyond the right
  // edge - and a click there is a silent no-op, which is exactly how a "click #N"
  // run stalls with no error. Validating the point turns that into "pick the next
  // match" instead.
  function clickableAt(el) {
    if (!el) return false;
    try { el.scrollIntoView({ block: "center", behavior: "instant" }); } catch (e) {}
    var r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return false;
    var cx = r.left + r.width / 2, cy = r.top + r.height / 2;
    if (cx < 2 || cy < 2 || cx > window.innerWidth - 2 || cy > window.innerHeight - 2) return false;
    var top = document.elementFromPoint(cx, cy);
    if (!top) return false;
    return top === el || el.contains(top) || top.contains(el);
  }

  var want = nth > 0 ? nth : 1;
  var usable = [];
  for (var k = 0; k < pool.length && usable.length < want; k++) {
    if (clickableAt(lb.element(pool[k].index))) usable.push(pool[k]);
  }
  var pick = usable[want - 1];
  var chosenEl = pick ? lb.element(pick.index) : null;
  var selUsed = null;

  // Fallback: when the numbered snapshot has no match - a virtualised list, or a
  // control the extension did not number - a CSS selector can still land the
  // click. The chosen node is tagged so the decision stays one stable reference
  // for the engine (`selector` in the decision, not `element`).
  if (!pick && selector) {
    var nodes = [];
    try { nodes = Array.prototype.slice.call(document.querySelectorAll(selector)); } catch (e) { nodes = []; }
    nodes.sort(function (a, b) {
      var ra = a.getBoundingClientRect(), rb = b.getBoundingClientRect();
      return (rb.width * rb.height) - (ra.width * ra.height);
    });
    for (var q = 0; q < nodes.length; q++) {
      if (clickableAt(nodes[q])) {
        nodes[q].setAttribute("data-laya-pick", "1");
        chosenEl = nodes[q];
        selUsed = '[data-laya-pick="1"]';
        pick = {
          index: null,
          role: chosenEl.getAttribute("role") || chosenEl.tagName.toLowerCase(),
          name: String(chosenEl.getAttribute("aria-label") || chosenEl.innerText || chosenEl.textContent || "").trim().slice(0, 60),
          href: chosenEl.href || null
        };
        break;
      }
    }
  }

  if (!pick) {
    throw new Error("no clickable element matched"
      + " operation=" + operation
      + " roles=[" + roles.join(",") + "]"
      + " match=[" + needles.join(",") + "]"
      + " selector=" + (selector || "-")
      + " nth=" + nth
      + " matched=" + pool.length + " clickable=" + usable.length
      + " | visible " + (roles.length ? roles.join("/") : "elements") + ": " + nearby.join(" | "));
  }

  lb.highlight();                                // draw the numbered badges
  var el = chosenEl || lb.element(pick.index);
  if (el && el.scrollIntoView) el.scrollIntoView({ block: "center" });
  // Identity, not a label. The snapshot number is good only *this* round: the
  // extension re-snapshots when the DOM mutates, so the number the engine
  // resolves a beat later (after it brings the tab to the front) can point at a
  // different element - that is how a click meant for 搜索 once landed on 收藏夹.
  // Tag the node we just validated and hand the engine a selector for *it*: the
  // click then reaches this element, or fails loudly when the site has replaced
  // it, instead of acting on whatever inherited the old number. The decision
  // still carries the number, so the journal and the overlay stay numbered.
  if (!selUsed && el) {
    try { el.setAttribute("data-laya-pick", "1"); selUsed = '[data-laya-pick="1"]'; } catch (e) { selUsed = null; }
  }
  // A site may force a *new* tab. Three mechanisms, all normalized when the
  // spec sets `stay_in_tab`, so the *real* CDP click lands in the driven tab:
  //   1. a link carrying target=_blank;
  //   2. a <button type=submit> inside a <form target=_blank> - Taobao's 搜索 is
  //      exactly this, and the site's own JS rewrites the form target to _blank
  //      after hydration. A form submit is a *browser-native* new-tab navigation,
  //      so it never runs window.open and patching that alone misses it;
  //   3. a JS handler that calls window.open (Taobao product cards, some buttons).
  // The click itself is unchanged - only where the navigation lands.
  var stayed = false;
  if (operation === "CLICK" && O.stay_in_tab) {
    if (el && el.tagName === "A" && el.getAttribute("href")) el.setAttribute("target", "_self");
    (function () {
      var form = el && (el.form || (el.closest ? el.closest("form") : null));
      if (form) form.setAttribute("target", "_self");
      // A site handler can reset target=_blank on submit; reassert it for the
      // click window so the navigation cannot escape this tab.
      var onsubmit = function (ev) {
        var f = ev.target;
        if (f && f.tagName === "FORM") f.setAttribute("target", "_self");
      };
      document.addEventListener("submit", onsubmit, true);
      window.setTimeout(function () { document.removeEventListener("submit", onsubmit, true); }, 8000);
      if (form) stayed = true;
    })();
    (function () {
      var original = window.open;
      var restore = function () { if (window.open === patched) window.open = original; };
      function patched(u) {
        if (typeof u === "string" && u) {
          restore();
          if (/^https?:/i.test(u) || u.charAt(0) === "/") { window.location.href = u; return null; }
        }
        return original.apply(window, arguments);
      }
      window.open = patched;
      window.setTimeout(restore, 8000);
      stayed = true;
    })();
  }

  var where = (pick.index === null || pick.index === undefined) ? selUsed : ("#" + pick.index);
  var why = "round " + (snap.version || 0) + ": " + operation + " " + where
    + " (" + pick.role + " \"" + String(pick.name || "").slice(0, 40) + "\")"
    + (text ? " with text \"" + text.slice(0, 40) + "\"" : "");
  if (O.note) why = String(O.note) + " — " + why;

  var out = {
    operation: operation,
    element: pick.index,
    selector: selUsed,
    name: pick.name,
    role: pick.role,
    href: pick.href || null,
    goal_probability: Number(O.goal_probability === undefined ? 0.75 : O.goal_probability),
    stuck_probability: Number(O.stuck_probability === undefined ? 0.1 : O.stuck_probability),
    rationale: why,
    note: O.note ? String(O.note) : null,
    stayed_in_tab: stayed,
    nearby: nearby,
    candidates: pick.index === null ? 1 : pool.length,
    resolved_by: pick.index === null ? "selector" : "snapshot",
    identity: selUsed ? "tag" : "index"
  };
  if (operation === "TYPE_TEXT") out.text = text;
  if (operation === "SELECT") out.value = text;
  return out;
})()
