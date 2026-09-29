// item.js — read one product-detail page and decide whether it is a
// *single-stick server DIMM*.
//
// The workflow's sweep op opens each product card in the owned tab and calls
// this to record a compact, comparable row: title, price, and the flags that
// matter for "单条服务器内存条" — a server/ECC/RDIMM memory versus a
// consumer kit — plus capacity and DDR5 frequency, and a single 0–20 score the
// sweep ranks by. All strings are upper-cased first so the ASCII markers
// (RECC/ECC/RDIMM/1RX4/2RX8) match however the seller typed them.
(function () {
  "use strict";
  var body = (document.body ? document.body.innerText : "") || "";
  var title = (document.title || "").replace(/[\-—]\s*(淘宝网|天猫|淘宝)\s*$/, "").trim();

  // price: the detail page's own price node first, else the first ¥ on the page
  var priceText = "";
  var pe = document.querySelector("[class*=highlightPrice],[class*=normalPrice],[class*=priceWrap],[class*=Price]");
  if (pe) priceText = (pe.innerText || "").trim();
  if (!priceText) {
    var m0 = body.match(/[¥￥]\s*([0-9]+(?:\.[0-9]+)?)/);
    if (m0) priceText = m0[0];
  }
  var pm = priceText.match(/([0-9]+(?:\.[0-9]+)?)/);
  var price = pm ? Number(pm[1]) : null;

  // haystack: title + the 参数信息 (spec) block + the first screen
  var specMatch = body.match(/参数信息([\s\S]{0,700})/);
  var hay = (title + " " + (specMatch ? specMatch[1] : "") + " " + body.slice(0, 1500)).toUpperCase();

  var serverWord = /服务器|工作站/.test(hay);
  var eccReg = /RECC|ECC|RDIMM|LRDIMM|REG\s*ECC|1RX4|2RX8|4RX4|4DRX4/.test(hay);
  var server = serverWord || eccReg;
  var single = /单条|单根/.test(hay);
  var kit = /套条|套装|双条|两条装|（2条）|\(2条\)|X2|×2/.test(hay);

  // capacity: the biggest ``\d{2,3}G`` that is not a frequency (…MHz / …MT)
  var capRe = /(\d{2,3})\s*G(?:B)?(?![HZ])/g, cap = 0, cm;
  while ((cm = capRe.exec(hay)) !== null) {
    var v = parseInt(cm[1], 10);
    if (v > cap && v <= 512) cap = v;
  }
  var freqM = hay.match(/(4800|5000|5200|5400|5600|5800|6000|6200|6400|6600|6800|7000|7200)/);
  var freq = freqM ? Number(freqM[1]) : null;

  // a listing that spans several capacities is a multi-SKU page, not one item
  var variants = {}, m3, re3 = /(\d{2,3})\s*G(?:B)?(?![HZ])/gi;
  while ((m3 = re3.exec(title)) !== null) variants[m3[1]] = 1;
  var vcount = Object.keys(variants).length;

  var score = 0;
  if (server) score += 5;        // 服务器 / RECC / ECC / RDIMM / 1Rx4 / 2Rx8
  if (eccReg) score += 3;
  if (serverWord) score += 2;
  if (single) score += 3;        // explicit 单条 / 单根
  var stick = single || (server && !kit);   // a server DIMM is a single stick
  if (server && !kit) score += 1;
  if (kit) score -= 8;           // 套条 / 套装 / 双条 = not a single stick
  score += cap >= 64 ? 4 : cap >= 32 ? 3 : cap >= 16 ? 2 : cap >= 8 ? 1 : 0;
  if (freq && freq >= 5600) score += 1;
  if (vcount >= 3) score -= 2;

  // Taobao answers too-fast navigation with a *punish* wall: either a
  // J_MIDDLEWARE_FRAME_WIDGET overlay (slider / click / drag-drop captcha) or a
  // whole-page "验证码拦截". Report it so the sweep can cool down and retry
  // instead of recording a bogus product.
  var wall = !!document.querySelector(".J_MIDDLEWARE_FRAME_WIDGET")
    || /验证码|拦截|访问过于频繁|访问太频繁/.test(document.title || "")
    || /punish|x5sec|captcha/i.test(location.href);

  return {
    url: location.href,
    wall: wall,
    title: title.slice(0, 140),
    price: price,
    price_text: priceText.slice(0, 30),
    server: server,
    ecc_reg: eccReg,
    server_word: serverWord,
    single: single,
    stick: stick,
    kit: kit,
    variants: vcount,
    capacity_gb: cap || null,
    freq_mhz: freq,
    score: score,
    spec_excerpt: (specMatch ? specMatch[1] : "").replace(/\s+/g, " ").trim().slice(0, 240)
  };
})()
