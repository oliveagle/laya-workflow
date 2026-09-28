// GitHub trending: one record per `article.Box-row`. GitHub renders late, so
// wait for the rows instead of trusting the first paint.
(async () => {
  const OPT = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const limit = (OPT.limit | 0) || 25;
  const clean = (s) => (s || '').replace(/[\s\u00a0]+/g, ' ').trim();
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const until = async (sel, ms) => {
    const t0 = Date.now();
    while (Date.now() - t0 < ms) {
      if (document.querySelector(sel)) return true;
      await sleep(200);
    }
    return false;
  };
  const num = (s) => {
    const m = clean(s).replace(/,/g, '').match(/([\d.]+)\s*([km])?/i);
    if (!m) return 0;
    let v = parseFloat(m[1]);
    const u = (m[2] || '').toLowerCase();
    if (u === 'k') v *= 1e3; else if (u === 'm') v *= 1e6;
    return Math.round(v);
  };
  await until('article.Box-row', 25000);
  const full = (href) => { try { return new URL(href, location.href).href; } catch (e) { return href || ''; } };
  const results = [];
  const rows = document.querySelectorAll('article.Box-row');
  for (let i = 0; i < rows.length && results.length < limit; i++) {
    const row = rows[i];
    const a = row.querySelector('h2 a');
    if (!a) continue;
    const name = clean(a.textContent).replace(/\s*\/\s*/, '/');
    const p = name.split('/');
    const desc = row.querySelector('p');
    const lang = row.querySelector('[itemprop="programmingLanguage"]');
    const stars = row.querySelector('a[href$="/stargazers"]');
    const forks = row.querySelector('a[href$="/forks"]');
    const today = row.querySelector('span.float-sm-right, span.d-inline-block.float-sm-right');
    results.push({
      rank: results.length + 1,
      name: name,
      owner: p[0] || '',
      repo: p[1] || '',
      url: full(a.getAttribute('href')),
      description: clean(desc ? desc.textContent : ''),
      language: clean(lang ? lang.textContent : ''),
      stars: num(stars ? stars.textContent : ''),
      forks: num(forks ? forks.textContent : ''),
      stars_today: num(today ? today.textContent : ''),
      stars_today_text: clean(today ? today.textContent : ''),
    });
  }
  return { count: results.length, results: results, page_title: clean(document.title) };
})()
