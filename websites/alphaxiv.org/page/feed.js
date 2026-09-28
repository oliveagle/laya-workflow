(async () => {
  const OPTS = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const want = Math.max(1, OPTS.count | 0 || 10);
  const sort = OPTS.sort || 'Hot';
  const interval = OPTS.interval || '7 Days';
  const pageSize = Math.min(100, Math.max(1, (OPTS.page_size | 0) || Math.min(100, want)));
  const maxPages = (OPTS.max_pages | 0) || (Math.ceil(want / pageSize) + 3);
  const out = [];
  const seen = new Set();
  let pages = 0;
  for (let page = 1; page <= maxPages && out.length < want; page++) {
    const url = 'https://api.alphaxiv.org/papers/v3/feed'
      + '?pageNum=' + page
      + '&pageSize=' + pageSize
      + '&sort=' + encodeURIComponent(sort)
      + '&interval=' + encodeURIComponent(interval)
      + '&linkBlogs=true&topics=%5B%5D';
    let payload;
    try {
      const r = await fetch(url, { headers: { accept: 'application/json' } });
      if (!r.ok) break;
      payload = await r.json();
    } catch (e) { break; }
    const papers = (payload && payload.papers) || [];
    if (!papers.length) break;
    pages++;
    for (const paper of papers) {
      const id = paper.universal_paper_id || paper.canonical_id;
      if (!id) continue;
      const abs = 'https://www.alphaxiv.org/abs/' + id;
      if (seen.has(abs)) continue;
      seen.add(abs);
      out.push({ url: abs, title: (paper.title || '').slice(0, 200) });
      if (out.length >= want) break;
    }
  }
  return { count: out.length, links: out, pages: pages, sort: sort, interval: interval };
})()
