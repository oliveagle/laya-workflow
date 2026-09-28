// PyPI search results: one record per `a.package-snippet`.
(() => {
  const OPT = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const limit = (OPT.limit | 0) || 10;
  const clean = (s) => (s || '').replace(/[\s\u00a0]+/g, ' ').trim();
  const full = (href) => { try { return new URL(href, location.href).href; } catch (e) { return href || ''; } };
  const results = [];
  const items = document.querySelectorAll('a.package-snippet');
  for (let i = 0; i < items.length && results.length < limit; i++) {
    const a = items[i];
    const name = a.querySelector('.package-snippet__name');
    const ver = a.querySelector('.package-snippet__version');
    const desc = a.querySelector('.package-snippet__description');
    const t = a.querySelector('time[datetime]');
    results.push({
      rank: results.length + 1,
      name: clean(name ? name.textContent : '') || clean(a.textContent),
      version: ver ? clean(ver.textContent) : '',
      description: clean(desc ? desc.textContent : ''),
      updated: t ? (t.getAttribute('datetime') || clean(t.textContent)) : '',
      url: full(a.getAttribute('href')),
    });
  }
  return { count: results.length, results: results, page_title: clean(document.title) };
})()
