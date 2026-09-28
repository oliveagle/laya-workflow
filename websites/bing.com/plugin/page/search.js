// Bing organic results: one record per `li.b_algo`.
(() => {
  const OPT = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const limit = (OPT.limit | 0) || 10;
  const clean = (s) => (s || '').replace(/[\s\u00a0]+/g, ' ').trim();
  const results = [];
  const items = document.querySelectorAll('li.b_algo');
  for (let i = 0; i < items.length && results.length < limit; i++) {
    const li = items[i];
    const a = li.querySelector('h2 a') || li.querySelector('a[href^="http"]');
    if (!a) continue;
    const href = a.getAttribute('href') || '';
    if (!/^https?:/i.test(href)) continue;
    const snip = li.querySelector('.b_caption p, .b_lineclamp2, .b_algoSlug, p');
    const cite = li.querySelector('cite, .b_attribution');
    results.push({
      rank: results.length + 1,
      title: clean(a.textContent),
      url: href,
      site: clean(cite ? cite.textContent : ''),
      snippet: clean(snip ? snip.textContent : ''),
    });
  }
  return { count: results.length, results: results, page_title: clean(document.title) };
})()
