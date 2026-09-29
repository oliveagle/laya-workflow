// cards.js — read the product cards on a Taobao/Tmall search-results page.
//
// Returns the organic product links (item.taobao.com / detail.tmall.com /
// item.tmall.com `/item.htm?id=…`), de-duplicated by the item id and kept in
// DOM order, each with the title and the first ¥ price shown on its card. The
// sweep op walks this list: one card, one detail page, one at a time.
(function () {
  "use strict";
  var anchors = document.querySelectorAll("a[href]");
  var out = [], seen = {};
  var re = /(?:item\.taobao\.com|detail\.tmall\.com|item\.tmall\.com)\/item\.htm/i;
  for (var i = 0; i < anchors.length; i++) {
    var a = anchors[i];
    var raw = a.getAttribute("href") || "";
    if (!re.test(raw)) continue;
    var idm = raw.match(/[?&]id=(\d+)/);
    var key = idm ? idm[1] : raw;
    if (seen[key]) continue;
    seen[key] = 1;
    var text = (a.innerText || a.textContent || "").replace(/\r/g, "");
    var lines = text.split("\n").map(function (s) { return s.trim(); }).filter(Boolean);
    var pm = text.match(/[¥￥]\s*([0-9]+(?:\.[0-9]+)?)/);
    out.push({
      i: out.length,
      id: key,
      // canonical form: drop the personalised tracking query, keep the id
      href: "https://" + (raw.indexOf("tmall") >= 0 ? "detail.tmall.com" : "item.taobao.com") + "/item.htm?id=" + key,
      title: (lines.length ? lines[0] : "").slice(0, 120),
      price: pm ? Number(pm[1]) : null
    });
  }
  return { url: location.href, title: document.title, n: out.length, cards: out };
})()
