(() => new Promise((resolve) => {
  const limit = (__LAYA_OPTS__.limit | 0) || 10;
  const deadline = Date.now() + ((__LAYA_OPTS__.timeout_ms | 0) || 12000);
  const collect = () => {
    const seen = new Set();
    const out = [];
    for (const a of document.querySelectorAll('a[href*="/abs/"]')) {
      const raw = a.href || '';
      const u = raw.split('#')[0].split('?')[0];
      if (!/^https?:\/\/(www\.)?alphaxiv\.org\/abs\//i.test(u)) continue;
      if (seen.has(u)) continue;
      seen.add(u);
      const card = a.closest('article, li, div') || a;
      const text = ((card.innerText || a.innerText || '') + '').replace(/\s+/g, ' ').trim();
      out.push({ url: u, title: text.slice(0, 200) });
      if (out.length >= limit) break;
    }
    return out;
  };
  const tick = () => {
    const links = collect();
    if (links.length >= limit || Date.now() > deadline) {
      resolve({ count: links.length, links: links });
    } else {
      setTimeout(tick, 400);
    }
  };
  tick();
}))()
