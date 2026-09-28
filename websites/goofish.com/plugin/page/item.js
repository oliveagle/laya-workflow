// ─────────────────────────────────────────────────────────────────────────────
// goofish (闲鱼) item-detail collector.
//
// Reads one `/item?id=<id>` page into a structured record: price, the want /
// view counters, the seller's tag strip, the attribute table, the description,
// and the picture carousel.
//
// Two goofish-specific things this has to survive:
//
// 1. The detail page is a SPA. `document.body.innerText.length` is already
//    large while it still renders the site footer, so "has content" must be
//    keyed on the item's own block — waiting on body text returns a page with
//    nothing but the footer.
// 2. Class names carry a per-deploy CSS-module hash, so every lookup matches
//    the stable prefix only.
//
// A sold / removed / wrong-id item never renders the item block at all, so that
// case is reported as an explicit `status` instead of a record full of nulls.
// ─────────────────────────────────────────────────────────────────────────────

(() => new Promise((resolve) => {
  const OPTS = (typeof __LAYA_OPTS__ !== 'undefined' && __LAYA_OPTS__) || {};
  const DEADLINE = Date.now() + ((OPTS.timeout_ms | 0) || 25000);
  const WANT_IMAGES = OPTS.max_images || 8;

  const byPrefix = (root, prefix) => {
    if (!root) return null;
    for (const el of root.querySelectorAll('*')) {
      const cls = el.getAttribute('class') || '';
      if (cls === prefix || cls.indexOf(prefix + '--') === 0) return el;
    }
    return null;
  };

  const q = (root, prefix) => (root || document).querySelector(
    '[class^="' + prefix + '--"], [class="' + prefix + '"]'
  );

  const text = (el) => (el && el.textContent ? el.textContent.replace(/\s+/g, ' ').trim() : '');

  const absolutize = (src) => {
    let u = (src || '').trim();
    if (u.indexOf('//') === 0) u = 'https:' + u;
    return u;
  };

  const parsePrice = (raw) => {
    const s = (raw || '').replace(/[,\s]/g, '');
    const m = s.match(/-?\d+(?:\.\d+)?/);
    return m ? parseFloat(m[0]) : null;
  };

  // The detail price is not always a single number:
  //   "19800"          a normal price
  //   "9180 - 15480"   a coupon price and the price before it — the buyer pays
  //                    the first, so that is the price and the second is kept
  //                    as the original
  //   "1.98万"         the same 万 abbreviation the feed uses, though here it
  //                    normally arrives already expanded
  const parseMoney = (raw, magnitudeText) => {
    const s = (raw || '').replace(/[,\s]/g, '');
    const range = s.match(/(-?\d+(?:\.\d+)?)[-~～至](\d+(?:\.\d+)?)/);
    let price;
    let original = null;
    if (range) {
      price = parseFloat(range[1]);
      original = parseFloat(range[2]);
    } else {
      price = parsePrice(s);
    }
    const m = (magnitudeText || '').trim();
    if (price !== null && (m === '万' || m === '千')) {
      const k = m === '万' ? 10000 : 1000;
      price *= k;
      if (original !== null) original *= k;
    }
    // Rounded for the same reason as the feed: 1.38万 is not exactly 13800.
    if (price !== null) price = Math.round(price * 100) / 100;
    if (original !== null) original = Math.round(original * 100) / 100;
    return { price: price, original_price: original };
  };

  // "4万浏览" is forty thousand views, not four.
  const parseCount = (raw) => {
    const s = (raw || '').replace(/[,\s]/g, '');
    const m = s.match(/(\d+(?:\.\d+)?)(万|千)?/);
    if (!m) return null;
    let n = parseFloat(m[1]);
    if (m[2] === '万') n *= 10000;
    if (m[2] === '千') n *= 1000;
    return n;
  };

  const itemId = () => {
    const m = (location.href || '').match(/[?&]id=(\d+)/);
    return m ? m[1] : '';
  };

  // "苹果11 128g 黑色_闲鱼" → "苹果11 128g 黑色"
  const docTitle = () => {
    const t = (document.title || '').trim();
    return t.replace(/[_-]\s*闲鱼\s*$/, '').trim() || '';
  };

  const SOLD_HINTS = ['该商品已经下架', '该宝贝已下架', '商品不存在', '宝贝不存在', '页面不存在', '该商品已被删除', '已失效'];
  const LOGIN_HINTS = ['请登录', '登录后查看', '扫码登录'];

  const statusOf = (pageText) => {
    if (pageText.indexOf('该商品已经下架') >= 0 || pageText.indexOf('该宝贝已下架') >= 0
        || pageText.indexOf('商品不存在') >= 0 || pageText.indexOf('宝贝不存在') >= 0
        || pageText.indexOf('页面不存在') >= 0 || pageText.indexOf('该商品已被删除') >= 0) {
      return 'gone';
    }
    for (const h of SOLD_HINTS) if (pageText.indexOf(h) >= 0) return 'gone';
    for (const h of LOGIN_HINTS) if (pageText.indexOf(h) >= 0) return 'login_required';
    return 'on_sale';
  };

  const readLabels = (root) => {
    const box = q(root, 'labels');
    const out = {};
    if (!box) return out;
    for (const row of box.querySelectorAll('[class^="item--"]')) {
      // The label glyphs are one <div> per character ("品" "牌"), so the key is
      // the concatenation of the label node's text.
      const key = text(row.querySelector('[class^="label--"]'));
      const value = text(row.querySelector('[class^="value--"]'));
      if (key && value) out[key] = value;
    }
    return out;
  };

  const readImages = () => {
    const car = q(document, 'carousel') || q(document, 'item-main-window-list');
    if (!car) return [];
    const seen = new Set();
    const out = [];
    for (const img of car.querySelectorAll('img')) {
      const src = absolutize(img.getAttribute('src') || img.getAttribute('data-src') || '');
      if (!src || seen.has(src)) continue;
      // The carousel also renders its own current-slide clone; dedupe by URL
      // is enough because goofish reuses one CDN URL per picture.
      if (/tps-2-2|1x1/i.test(src)) continue;
      seen.add(src);
      out.push(src);
      if (out.length >= WANT_IMAGES) break;
    }
    return out;
  };

  const readSeller = () => {
    const nick = text(q(document, 'item-user-info-nick'));
    const avatarBox = q(document, 'item-user-info-avatar');
    const avatarEl = avatarBox ? avatarBox.querySelector('img') : null;
    const intro = q(document, 'item-user-info-intro');
    const tags = [];
    if (intro) {
      for (const el of intro.querySelectorAll('[class^="item-user-info-label--"]')) {
        const t = text(el);
        if (t) tags.push(t);
      }
    }
    const joined = tags.join(' ');
    const years = joined.match(/来闲鱼\s*(\d+)\s*年/);
    const sold = joined.match(/卖出\s*(\d+)\s*件/);
    const credit = joined.match(/好评率\s*(\d+(?:\.\d+)?)\s*%/);
    // The first tag is the city ("揭阳"); the rest are the account badges.
    return {
      nick: nick,
      location: tags.length ? tags[0] : '',
      tags: tags.slice(1),
      joined_years: years ? parseInt(years[1], 10) : null,
      sold_count: sold ? parseInt(sold[1], 10) : null,
      credit_rate: credit ? parseFloat(credit[1]) : null,
      avatar: absolutize(avatarEl ? avatarEl.getAttribute('src') : ''),
    };
  };

  const state = { ok: false, status: 'unknown', error: '' };

  const read = () => {
    const pageText = (document.body ? document.body.innerText : '') || '';
    const main = q(document, 'item-main-info');
    const status = main ? 'on_sale' : statusOf(pageText);
    if (!main) {
      state.ok = false;
      state.status = status;
      state.url = location.href;
      state.id = itemId();
      state.title = docTitle();
      state.error = status === 'on_sale'
        ? 'the item block never rendered'
        : 'item is ' + status;
      return;
    }

    const priceWrap = q(main, 'value') || main;
    const number = q(main, 'price');
    const decimal = q(main, 'decimal');
    const wantBox = q(main, 'want');
    const wantText = text(wantBox);
    const shipping = text(q(main, 'post'));

    state.ok = true;
    state.status = 'on_sale';
    state.url = location.href;
    state.id = itemId();
    state.title = docTitle();
    // The 万 span, if any, lives next to the price rather than inside it.
    const magnitude = byPrefix(main, 'magnitude');
    const money = parseMoney(text(number) + text(decimal), magnitude ? text(magnitude) : '');
    state.price = money.price;
    state.original_price = money.original_price;
    state.price_text = text(priceWrap);
    const fans = q(main, 'fans');
    state.price_note = fans ? text(fans) : '';
    const discounts = q(main, 'discounts');
    state.discount_text = discounts ? text(discounts) : '';
    const wantMatch = wantText.match(/(\d+(?:\.\d+)?)\s*(万|千)?\s*人想要/);
    state.want_count = wantMatch ? parseCount(wantMatch[1] + (wantMatch[2] || '')) : null;
    const views = wantText.match(/(\d+(?:\.\d+)?)\s*(万|千)?\s*浏览/);
    state.views = views ? views.length ? parseCount(views[1] + (views[2] || '')) : null : null;
    state.shipping = shipping;
    state.description = text(q(document, 'desc'));
    state.labels = readLabels(document);
    state.images = readImages();
    state.seller = readSeller();
    state.card = text(q(main, 'card'));
    state.collected_at = Date.now();
  };

  const run = async () => {
    let waited = 0;
    while (Date.now() < DEADLINE && waited < DEADLINE) {
      read();
      if (state.ok || state.status === 'gone' || state.status === 'login_required') {
        // One settling pass so the carousel and the description land too.
        await new Promise((r) => setTimeout(r, 600));
        read();
        resolve(state);
        return;
      }
      await new Promise((r) => setTimeout(r, 500));
      waited += 500;
    }
    read();
    if (!state.ok && !state.error) state.error = 'timed out waiting for the item block';
    resolve(state);
  };

  run().catch((e) => {
    state.ok = false;
    state.error = '' + (e && e.message ? e.message : e);
    resolve(state);
  });
}))()
