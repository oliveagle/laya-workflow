// observe.js — read the owned tab as a numbered table.
//
// Draws the extension's numbered badges (so the page itself shows the 序号 the
// planner sees) and returns a compact, bounded summary: url, title, how many
// numbered elements there are, the first screen of them, and price-like
// labels. Used by the tour to record "what the page looked like" without
// dragging the whole element table into the journal.
(function () {
  "use strict";
  var lb = window.__layaBrowser;
  if (!lb || typeof lb.snapshot !== "function") {
    throw new Error("laya-browser extension is not loaded in this tab (no window.__layaBrowser)");
  }
  var snap = lb.snapshot(0, true);
  var drawn = lb.highlight();
  var rows = snap.elements || [];
  var sample = rows.slice(0, 30).map(function (r) {
    return { index: r.index, role: r.role, name: String(r.name || "").slice(0, 60) };
  });
  var money = [], seen = {};
  for (var i = 0; i < rows.length && money.length < 8; i++) {
    var n = String(rows[i].name || "").replace(/\s+/g, " ").trim();
    if (/^[¥￥]\s?\d/.test(n) && !seen[n]) { seen[n] = 1; money.push(n.slice(0, 40)); }
  }
  return {
    url: snap.url,
    title: snap.title,
    elements: rows.length,
    badges: drawn === true,
    sample: sample,
    money: money,
    price_hint: money.length ? money[0] : null
  };
})()
