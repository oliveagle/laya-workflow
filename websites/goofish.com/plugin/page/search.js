// ─────────────────────────────────────────────────────────────────────────────
// goofish (闲鱼) search-result collector.
//
// One `Runtime.evaluate` covers the whole listing: wait for the feed to
// hydrate, walk the site's own pagination, and return one row per card.
//
// Why the pagination is clicked instead of paged by URL: goofish ignores
// `?page=` / `?sort=` / `?priceMin=` on /search — the same 30 ids come back
// for every value (verified against the live site). The results are a React
// grid driven by in-page state, so "next page" means clicking the number box.
//
// Class names are CSS-module hashes (`row1-wrap-title--qIlOySTh`) that change
// on every deploy, so nothing here matches a full class: everything is matched
// on the stable prefix before the `--`.
// ─────────────────────────────────────────────────────────────────────────────

(() => new Promise((resolve) => {
  const OPTS = (typeof __LAYA_OPTS__ !== 'undefined' && __LAYA_OPTS__) || {};
  const LIMIT = OPTS.limit || 30;
  const MAX_PAGES = Math.max(1, OPTS.max_pages || 1);
  const DEADLINE = Date.now() + ((OPTS.timeout_ms | 0) || 25000);
  const CARD_SEL = 'a[href*="/item?id="]';

  // First element whose class *starts* with `prefix--`, or the bare name.
  // `prefix` never contains the hash, so a redeploy cannot break this.
  const byPrefix = (root, prefix) => {
    if (!root) return null;
    for (const el of root.querySelectorAll('*')) {
      const cls = el.getAttribute('class') || '';
      if (cls === prefix || cls.indexOf(prefix + '--') === 0) return el;
    }
    return null;
  };

  const text = (el) => (el && el.textContent ? el.textContent.replace(/\s+/g, ' ').trim() : '');

  // "¥948.90" / "948" / "价格面议" → number | null, plus the raw string.
  const parsePrice = (raw) => {
    const s = (raw || '').replace(/[,\s]/g, '');
    if (!s) return null;
    const m = s.match(/-?\d+(?:\.\d+)?/);
    return m ? parseFloat(m[0]) : null;
  };

  // goofish abbreviates four-figure-and-up prices in the feed: the card renders
  // `¥1.98万` as number "1" + decimal ".98" with a sibling <span>万</span>. Read
  // without that span the price comes back as 1.98 for a ¥19,800 camera — an
  // error a price monitor would then report as a 99.99% drop. Verified against
  // the same item's detail page, which shows the expanded 19800.
  const applyMagnitude = (value, magnitudeText) => {
    if (value === null) return null;
    const m = (magnitudeText || '').trim();
    // Rounded because `1.38 * 10000` is 13799.999999999998 in binary floating
    // point, and a price of 13799.999999999998 in a report is a bug report.
    if (m === '万') return Math.round(value * 10000 * 100) / 100;
    if (m === '千') return Math.round(value * 1000 * 100) / 100;
    return Math.round(value * 100) / 100;
  };

  // "35人想要" → 35; "¥5555" → the struck-through original; "累计降价12%" → 12.
  // The price-desc slot is overloaded on goofish, so classify rather than
  // trusting one regex.
  const parsePriceDesc = (raw) => {
    const s = (raw || '').trim();
    if (!s) return { want_count: null, original_price: null, drop_pct: null };
    const want = s.match(/(\d+)\s*人想要/);
    const drop = s.match(/(\d+(?:\.\d+)?)\s*%/);
    const yuan = s.match(/[¥￥]\s*([\d.]+)/);
    return {
      want_count: want ? parseInt(want[1], 10) : null,
      original_price: yuan ? parseFloat(yuan[1]) : null,
      drop_pct: drop && !yuan ? parseFloat(drop[1]) : null,
    };
  };

  const absolutize = (src) => {
    if (!src) return '';
    let u = src.trim();
    if (u.indexOf('//') === 0) u = 'https:' + u;
    return u;
  };

  // The feed ships every card image as a 2×2 transparent GIF (`tps-2-2`) and
  // swaps in the real picture only once an IntersectionObserver fires. Scrolling
  // the feed does not reliably trigger it (verified: after scrolling a full page
  // of cards every <img src> was still the 2×2 stub), and the DOM carries no
  // data-src / srcset / background-image to fall back on.
  //
  // The real URLs are in the React fiber hanging off the <img>, and — crucially
  // — they are card-scoped: walking *up from the card* reaches the feed's own
  // props and returns every picture on the page, while walking up from the <img>
  // and stopping after five levels yields just this card's photo (level 2) and
  // seller avatar (level 4). So read the fiber from the image and stop.
  //
  // Everything degrades to the DOM src plus `image_placeholder: true`, so a
  // React change costs image URLs and nothing else.
  const PLACEHOLDER = /tps-2-2|1x1|blank/i;
  const MAX_FIBER_LEVELS = 5;

  const fiberMedia = (img) => {
    const key = Object.keys(img).find((k) => k.indexOf('__reactFiber$') === 0);
    if (!key) return { image: '', avatar: '' };
    const out = { image: '', avatar: '' };
    let node = img[key];
    let level = 0;
    while (node && level < MAX_FIBER_LEVELS) {
      const props = node.memoizedProps;
      if (props && typeof props === 'object') {
        let dumped = '';
        try { dumped = JSON.stringify(props); } catch (e) { dumped = ''; }
        if (dumped) {
          const found = dumped.match(/bao\/uploaded[^"\\]+/g) || [];
          for (const raw of found) {
            const url = 'https://img.alicdn.com/' + raw.replace(/\\/g, '');
            if (/-mtopupload/.test(url)) {
              if (!out.avatar) out.avatar = url;
            } else if (!out.image) {
              out.image = url;
            }
          }
        }
      }
      if (out.image && out.avatar) break;
      node = node.return;
      level += 1;
    }
    return out;
  };

  const readImage = (card) => {
    const img = card.querySelector('img');
    const fiber = img ? fiberMedia(img) : { image: '', avatar: '' };
    const src = absolutize(img ? (img.getAttribute('src') || img.getAttribute('data-src') || '') : '');
    const useFiber = fiber.image && fiber.image !== src;
    return {
      image: useFiber ? fiber.image : src,
      avatar: fiber.avatar || '',
      image_placeholder: useFiber ? false : (!src || PLACEHOLDER.test(src)),
    };
  };

  const cardId = (href) => {
    const m = (href || '').match(/[?&]id=(\d+)/);
    return m ? m[1] : '';
  };

  const readCard = (card) => {
    const href = card.getAttribute('href') || '';
    const row1 = byPrefix(card, 'row1-wrap-title');
    const row3 = byPrefix(card, 'row3-wrap-price');
    const priceWrap = row3 ? byPrefix(row3, 'price-wrap') : null;
    const number = priceWrap ? byPrefix(priceWrap, 'number') : null;
    const decimal = priceWrap ? byPrefix(priceWrap, 'decimal') : null;
    const desc = row3 ? row3.querySelector('[title]') : null;
    // A sibling of price-wrap, not a descendant of it.
    const magnitude = row3 ? byPrefix(row3, 'magnitude') : null;
    const seller = byPrefix(card, 'row4-wrap-seller');
    const sellerText = seller ? byPrefix(seller, 'seller-text') : null;
    // Scoped to the credit box: the seller wrapper itself carries title="广东",
    // so a bare querySelector('[title]') hands back the location instead.
    const creditBox = seller ? byPrefix(seller, 'credit-container') : null;
    const credit = creditBox ? creditBox.querySelector('[title]') : null;
    const cpv = Array.prototype.slice
      .call(card.querySelectorAll('[class^="cpv-wrap--"]'))
      .map((w) => text(byPrefix(w, 'cpv')))
      .filter(Boolean);
    const image = readImage(card);

    // The row1 title attribute is the untruncated text; the visible span is
    // ellipsised, so prefer the attribute and fall back to the span.
    const title = (row1 && (row1.getAttribute('title') || '').trim())
      || text(byPrefix(card, 'main-title'));
    const priceText = text(priceWrap);
    const desc2 = parsePriceDesc(desc ? (desc.getAttribute('title') || '') : '');

    return {
      id: cardId(href),
      url: absolutize(href),
      title: title,
      // number carries the integer part and decimal the fraction ("12" + ".90").
      price: priceWrap
        ? applyMagnitude(parsePrice(text(number) + text(decimal)), magnitude ? text(magnitude) : '')
        : null,
      price_text: priceText,
      want_count: desc2.want_count,
      original_price: desc2.original_price,
      drop_pct: desc2.drop_pct,
      tags: cpv,
      location: text(sellerText),
      seller_credit: credit ? (credit.getAttribute('title') || '') : '',
      image: image.image,
      image_placeholder: image.image_placeholder,
      avatar: image.avatar,
    };
  };

  const pageNumbers = () =>
    Array.prototype.slice
      .call(document.querySelectorAll('[class*="search-pagination-page-box--"]'))
      .filter((el) => /^\d+$/.test(text(el)));

  const activePage = () => {
    for (const el of pageNumbers()) {
      if (/page-box-active/.test(el.getAttribute('class') || '')) return text(el);
    }
    return '';
  };

  const clickPage = (want) => {
    const box = pageNumbers().find((el) => text(el) === String(want));
    if (!box) return false;
    box.click();
    return true;
  };

  // Pull every card currently mounted, skipping ids already collected: goofish
  // keeps the previous page's DOM around while the next one streams in.
  const collect = (out, seen) => {
    let added = 0;
    for (const card of document.querySelectorAll(CARD_SEL)) {
      const id = cardId(card.getAttribute('href') || '');
      if (!id || seen.has(id)) continue;
      const row = readCard(card);
      if (!row.title && row.price === null) continue;
      seen.add(id);
      out.push(row);
      added += 1;
      if (out.length >= LIMIT) break;
    }
    return added;
  };

  const scroller = () => document.querySelector('.page-search') || document.scrollingElement || document.body;

  const state = { ok: false, pages: [], rows: [], error: '' };

  const run = async () => {
    const seen = new Set();
    let stagnant = 0;

    for (let page = 1; page <= MAX_PAGES; page += 1) {
      const before = seen.size;

      // Lazy images: pull the whole feed past the viewport once so the rows we
      // read afterwards carry real pictures instead of the 1×1 stub.
      const box = scroller();
      const height = box.scrollHeight || 0;
      for (let y = 0; y < height; y += 600) {
        box.scrollTop = y;
        await new Promise((r) => setTimeout(r, 40));
      }
      box.scrollTop = 0;

      let waited = 0;
      while (seen.size === before && waited < 8000 && Date.now() < DEADLINE) {
        collect(state.rows, seen);
        if (seen.size > before) break;
        await new Promise((r) => setTimeout(r, 400));
        waited += 400;
      }
      collect(state.rows, seen);

      state.pages.push({ page: page, collected: seen.size - before, active: activePage() });
      if (state.rows.length >= LIMIT) break;
      if (Date.now() > DEADLINE) break;

      if (page < MAX_PAGES) {
        if (seen.size === before) stagnant += 1; else stagnant = 0;
        // Two pages in a row that add nothing means the listing is exhausted.
        if (stagnant >= 2) break;
        if (!clickPage(page + 1)) break;
        await new Promise((r) => setTimeout(r, 1200));
      }
    }

    state.ok = state.rows.length > 0;
    if (!state.ok) state.error = 'no item cards matched on the search page';
    resolve(state);
  };

  run().catch((e) => {
    state.ok = false;
    state.error = '' + (e && e.message ? e.message : e);
    resolve(state);
  });
}))()
