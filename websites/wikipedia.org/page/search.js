// Wikipedia search results: one record per `li.mw-search-result`.
(() => {
  const OPT = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const limit = (OPT.limit | 0) || 10;
  const clean = (s) => (s || '').replace(/[\s\u00a0]+/g, ' ').trim();
  const full = (href) => { try { return new URL(href, location.href).href; } catch (e) { return href || ''; } };
  const results = [];
  const items = document.querySelectorAll('li.mw-search-result');
  for (let i = 0; i < items.length && results.length < limit; i++) {
    const li = items[i];
    const a = li.querySelector('.mw-search-result-heading a');
    if (!a) continue;
    const snippet = li.querySelector('.searchresult');
    const data = li.querySelector('.mw-search-result-data');
    results.push({
      rank: results.length + 1,
      title: clean(a.textContent),
      url: full(a.getAttribute('href')),
      snippet: clean(snippet ? snippet.textContent : ''),
      meta: clean(data ? data.textContent : ''),
    });
  }
  return { count: results.length, results: results, page_title: clean(document.title) };
})()
