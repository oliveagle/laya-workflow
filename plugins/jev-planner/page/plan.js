// plan.js — the page-side half of the Jev planner.
//
// It runs inside the tab the workflow owns, reads the page, and returns ONE
// Jev-style decision:
//
//   { operation, element | selector, goal_probability, stuck_probability, … }
//
// The Rust `chrome_cdp` `agent_step` op then executes that decision with real
// CDP input. Keeping the policy here (which links count as "content", when the
// page counts as "read") means the loop never has to know a site's DOM.
//
// The workflow injects `window.__LAYA__ = { goal: "…" }` before this file.
(function () {
  "use strict";

  var OPT = (typeof window.__LAYA__ === "object" && window.__LAYA__) ? window.__LAYA__ : {};
  var ROUND_KEY = "__laya_jev_round";
  var SEEN_KEY = "__laya_jev_seen";

  // ── where we are in the page ─────────────────────────────────────────────
  var root = document.documentElement;
  var viewport = window.innerHeight || root.clientHeight || 800;
  var y = Math.round(window.scrollY || root.scrollTop || 0);
  var height = Math.max(root.scrollHeight, document.body ? document.body.scrollHeight : 0);
  var slack = Math.max(160, Math.round(viewport * 0.25));
  var atBottom = y + viewport >= height - slack;
  var textLen = document.body && document.body.innerText ? document.body.innerText.length : 0;
  var headings = document.querySelectorAll("h1,h2,h3").length;

  // ── round counter + "already followed" memory (same-origin, survives nav) ─
  var round = 0;
  try { round = parseInt(window.localStorage.getItem(ROUND_KEY) || "0", 10) || 0; } catch (e) {}
  round += 1;
  try { window.localStorage.setItem(ROUND_KEY, String(round)); } catch (e) {}

  var seen = {};
  try { seen = JSON.parse(window.localStorage.getItem(SEEN_KEY) || "{}") || {}; } catch (e) { seen = {}; }

  // ── candidate links: main content first, chrome last ─────────────────────
  var SCOPE_SELECTORS = [
    "#mw-content-text .mw-parser-output",
    "#mw-content-text",
    "main article",
    "article",
    "main",
    "[role=main]",
    "body"
  ];
  var SKIP_HREF = /^(#|javascript:|mailto:|tel:|data:)/i;
  var SKIP_TARGETS = /(special:|help:|file:|category:|talk:|user:|wikipedia:|portal:|template:|cite_note|action=edit)/i;
  var SKIP_IN = "nav, header, footer, aside, table, .navbox, .infobox, .sidebar, .reflist, .mw-editsection, .toc, .catlinks, [role=navigation]";

  function hrefOf(a) {
    var raw = a.getAttribute("href") || "";
    if (!raw || SKIP_HREF.test(raw) || SKIP_TARGETS.test(raw)) return null;
    var url;
    try { url = new window.URL(a.href, window.location.href); } catch (e) { return null; }
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    if (url.href.split("#")[0] === window.location.href.split("#")[0]) return null;
    return url.href;
  }

  function collect() {
    for (var s = 0; s < SCOPE_SELECTORS.length; s++) {
      var scope = document.querySelector(SCOPE_SELECTORS[s]);
      if (!scope) continue;
      var anchors = scope.querySelectorAll("a[href]");
      var out = [];
      for (var i = 0; i < anchors.length && out.length < 60; i++) {
        var a = anchors[i];
        var label = (a.textContent || "").replace(/\s+/g, " ").trim();
        if (label.length < 6 || label.length > 60) continue;
        if (a.closest(SKIP_IN)) continue;
        var href = hrefOf(a);
        if (!href) continue;
        out.push({ el: a, url: href, label: label, key: href.replace(/^https?:\/\//, "") });
      }
      if (out.length) return out;
    }
    return [];
  }

  var links = collect();
  var fresh = [];
  var followed = 0;
  for (var li = 0; li < links.length; li++) {
    if (seen[links[li].key]) followed += 1; else fresh.push(links[li]);
  }

  var goal = String(OPT.goal || "read this page through, then follow one link to go deeper");

  function mark(el) {
    var old = document.querySelectorAll("[data-laya-pick]");
    for (var i = 0; i < old.length; i++) old[i].removeAttribute("data-laya-pick");
    el.setAttribute("data-laya-pick", "1");
    try { el.scrollIntoView({ block: "center" }); } catch (e) {}
  }

  function decision(operation, fields) {
    var record = {
      round: round,
      goal: goal,
      url: window.location.href,
      title: document.title,
      operation: operation,
      selector: fields.selector === undefined ? null : fields.selector,
      element: fields.element === undefined ? null : fields.element,
      goal_probability: fields.goal_probability,
      stuck_probability: fields.stuck_probability,
      rationale: fields.rationale,
      page: {
        scroll_y: y,
        scroll_height: height,
        viewport: viewport,
        at_bottom: atBottom,
        text_len: textLen,
        headings: headings,
        links_total: links.length,
        links_fresh: fresh.length,
        links_followed: followed
      }
    };
    if (fields.next_url) record.next_url = fields.next_url;
    if (fields.next_label) record.next_label = fields.next_label;
    // The workflow appends this pre-rendered line to its journal: one JSON
    // object per round, so the audit trail is the same shape as the decision.
    record.line = JSON.stringify(record);
    return record;
  }

  if (!atBottom) {
    return decision("SCROLL_DOWN", {
      amount: Math.round(viewport * 0.85),
      goal_probability: 0.25,
      stuck_probability: 0.05,
      rationale: "round " + round + ": this page is not read through yet (y=" + y + " of " + Math.round(height) + ")"
    });
  }

  if (fresh.length) {
    var pick = fresh[0];
    mark(pick.el);
    try { seen[pick.key] = round; window.localStorage.setItem(SEEN_KEY, JSON.stringify(seen)); } catch (e) {}
    return decision("CLICK", {
      selector: '[data-laya-pick="1"]',
      goal_probability: 0.55,
      stuck_probability: 0.1,
      next_url: pick.url,
      next_label: pick.label,
      rationale: "round " + round + ": page read to the bottom; go deeper through \"" + pick.label + "\""
    });
  }

  return decision("DONE", {
    goal_probability: 0.9,
    stuck_probability: 0.45,
    rationale: "round " + round + ": bottom reached and every in-content link here has already been followed"
  });
})()
