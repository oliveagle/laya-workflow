(() => new Promise((resolve) => {
  const OPTS = (typeof __LAYA_OPTS__ !== 'undefined' && __LAYA_OPTS__) || {};
  const DEADLINE = Date.now() + ((OPTS.timeout_ms | 0) || 25000);
  const text = (el) => (el && el.textContent ? el.textContent.replace(/\s+/g, ' ').trim() : '');
  const byPrefix = (root, prefix) => {
    if (!root) return null;
    for (const el of root.querySelectorAll('*')) {
      const cls = el.getAttribute('class') || '';
      if (cls === prefix || cls.indexOf(prefix + '--') === 0) return el;
    }
    return null;
  };
  const pick = (root, prefix) => byPrefix(root || document, prefix);
  const parseNum = (raw) => {
    const m = String(raw || '').replace(/,/g, '').match(/\d+(?:\.\d+)?/);
    return m ? parseFloat(m[0]) : null;
  };
  const itemId = () => {
    const m = (location.href || '').match(/[?&]id=(\d+)/);
    return m ? m[1] : '';
  };
  const docTitle = () => {
    const t = (document.title || '').trim();
    return t.replace(/[-_]\s*(tmall\.com天猫|淘宝网|淘宝)\s*$/i, '').trim() || t;
  };
  const SOLD_HINTS = ['宝贝被删掉', '宝贝被删除', '该商品已被删除', '该宝贝已下架', '该商品已经下架', '商品不存在', '宝贝不存在', '页面不存在', '商品已下架', '宝贝已下架', '已失效'];
  const LOGIN_HINTS = ['请登录', '登录后查看', '扫码登录'];
  const BLOCKED_HINTS = ['bixi.alicdn.com/punish', 'action=wait', '安全验证', '滑动验证', '访问异常'];
  const statusOf = (pageText) => {
    const href = location.href || '';
    for (const h of BLOCKED_HINTS) {
      if (href.indexOf(h) >= 0 || pageText.indexOf(h) >= 0) return 'blocked';
    }
    for (const h of SOLD_HINTS) if (pageText.indexOf(h) >= 0) return 'gone';
    for (const h of LOGIN_HINTS) if (pageText.indexOf(h) >= 0) return 'login_required';
    return 'on_sale';
  };
  const moneyFrom = (label, parent, fallbackText) => {
    const raw = text(parent) || fallbackText || '';
    const value = parseNum(raw);
    return {
      price: value,
      price_text: raw,
      label: text(label),
    };
  };
  const read = () => {
    const body = document.body ? document.body.innerText : '';
    const pageText = body || '';
    const pageStatus = statusOf(pageText);
    // `bixi.alicdn.com/punish?...action=wait` is Taobao's transient wait page;
    // it normally redirects back to the detail page. Do not fail on contact —
    // only a page still blocked when the budget expires is a failure.
    if (pageStatus === 'blocked') {
      return {
        ok: false, status: 'blocked', url: location.href, id: itemId(), title: docTitle(),
        error: 'taobao risk-control page still active after the read budget: '
          + (location.href || '').slice(0, 240),
        price: null, original_price: null, price_text: '', collected_at: Date.now(),
      };
    }
    const highlight = pick(document, 'highlightPrice');
    const sub = pick(document, 'subPrice');
    const status = highlight || sub ? 'on_sale' : pageStatus;
    const state = {
      ok: !!highlight || !!sub,
      status: status,
      url: location.href,
      id: itemId(),
      title: docTitle(),
      error: '',
      price: null,
      original_price: null,
      price_text: '',
      price_note: '',
      discount_text: '',
      want_count: null,
      views: null,
      shipping: '',
      description: '',
      labels: {},
      images: [],
      seller: {},
      card: '',
      collected_at: Date.now(),
    };
    if (!state.ok) {
      state.error = status === 'on_sale' ? 'the price block never rendered' : 'item is ' + status;
      return state;
    }

    const highlightLabel = highlight ? byPrefix(highlight, 'title') : null;
    const subLabel = sub ? byPrefix(sub, 'title') : null;
    const hi = moneyFrom(highlightLabel, highlight, '');
    const old = moneyFrom(subLabel, sub, '');
    state.price = hi.price;
    state.original_price = old.price;
    state.price_text = hi.price_text || old.price_text;
    state.price_note = hi.label || '';
    const soldSpan = Array.from(document.querySelectorAll('span'))
      .find((el) => /^\s*已售\s*[\d.,]+(?:万)?\+?\s*$/.test(text(el)));
    const soldText = text(soldSpan);
    const soldMatch = soldText.match(/已售\s*([\d.,]+)(万)?/);
    if (soldMatch) {
      let n = parseFloat(soldMatch[1].replace(/,/g, ''));
      if (soldMatch[2] === '万') n *= 10000;
      state.want_count = Math.round(n);
    }
    const titleEl = pick(document, 'title');
    if (titleEl && text(titleEl).length >= 8) state.card = text(titleEl);
    const sellerEl = pick(document, 'shopNameText') || pick(document, 'sellerNick') || pick(document, 'seller');
    const seller = sellerEl ? text(sellerEl) : '';
    state.seller = { nick: seller, location: '', tags: [], joined_years: null, sold_count: state.want_count, credit_rate: null, avatar: '' };
    const desc = pick(document, 'desc') || pick(document, 'description') || pick(document, 'itemDesc');
    state.description = desc ? text(desc).slice(0, 4000) : '';
    const images = Array.from(document.querySelectorAll('img'))
      .map((img) => (img.getAttribute('src') || img.getAttribute('data-src') || '').trim())
      .filter((src) => src && !/1x1|blank|placeholder/i.test(src))
      .slice(0, 8);
    state.images = images;
    state.collected_at = Date.now();
    return state;
  };

  const run = async () => {
    let waited = 0;
    while (Date.now() < DEADLINE && waited < DEADLINE) {
      const state = read();
      // `blocked` intentionally stays in the poll loop: the wait page is meant
      // to resolve automatically. It is returned only by the timeout branch.
      if (state.ok || state.status === 'gone' || state.status === 'login_required') {
        await new Promise((r) => setTimeout(r, 500));
        const settled = read();
        resolve(settled);
        return;
      }
      await new Promise((r) => setTimeout(r, 500));
      waited += 500;
    }
    const state = read();
    if (!state.ok && !state.error) state.error = 'timed out waiting for the price block';
    resolve(state);
  };

  run().catch((e) => {
    resolve({
      ok: false, status: 'unknown', error: '' + (e && e.message ? e.message : e),
      url: location.href, id: itemId(), title: docTitle(),
      price: null, original_price: null, price_text: '', collected_at: Date.now(),
    });
  });
}))()
