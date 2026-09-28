// A GitHub repository page: identity + counters from stable hooks.
(async () => {
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
  const meta = (p) => {
    const e = document.querySelector('meta[property="' + p + '"], meta[name="' + p + '"]');
    return e ? (e.getAttribute('content') || '') : '';
  };
  await until('article.markdown-body, [data-testid="repository-container-header"]', 25000);
  const slug = location.pathname.replace(/^\//, '').replace(/\/$/, '');
  const parts = slug.split('/');
  const star = document.querySelector('#repo-stars-counter-star');
  const fork = document.querySelector('#repo-network-counter');
  return {
    slug: slug,
    owner: parts[0] || '',
    repo: parts[1] || '',
    description: clean(meta('og:description')),
    title: clean(meta('og:title')),
    stars: num(star ? star.textContent : ''),
    forks: num(fork ? fork.textContent : ''),
    readme_present: !!document.querySelector('article.markdown-body'),
    url: 'https://github.com/' + slug,
  };
})()
