// arXiv search results: one record per `li.arxiv-result`.
(() => {
  const OPT = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const limit = (OPT.limit | 0) || 10;
  const clean = (s) => (s || '').replace(/[\s\u00a0]+/g, ' ').trim();
  const full = (href) => {
    if (!href) return '';
    if (/^https?:\/\//i.test(href)) return href;
    return 'https://arxiv.org' + (href.charAt(0) === '/' ? href : '/' + href);
  };
  const results = [];
  const items = document.querySelectorAll('li.arxiv-result');
  for (let i = 0; i < items.length && results.length < limit; i++) {
    const li = items[i];
    const idA = li.querySelector('p.list-title a');
    const absHref = idA ? (idA.getAttribute('href') || '') : '';
    const m = absHref.match(/abs\/([^\/?#]+)/);
    const titleEl = li.querySelector('p.title');
    const authorsEl = li.querySelector('p.authors');
    const fullEl = li.querySelector('.abstract-full');
    const shortEl = li.querySelector('.abstract-short');
    const pdfA = li.querySelector('.list-title a[href*="/pdf/"]');
    const cats = [];
    const tagEls = li.querySelectorAll('.tags .tag');
    for (let j = 0; j < tagEls.length; j++) cats.push(clean(tagEls[j].textContent));
    let authors = authorsEl ? clean(authorsEl.textContent) : '';
    if (authors.indexOf('Authors:') === 0) authors = authors.slice(8).trim();
    const absEl = fullEl || shortEl;
    let abs = absEl ? clean(absEl.textContent) : '';
    if (abs.indexOf('Abstract:') === 0) abs = abs.slice(9).trim();
    results.push({
      rank: results.length + 1,
      arxiv_id: m ? m[1] : '',
      title: titleEl ? clean(titleEl.textContent) : '',
      authors: authors,
      abstract: abs,
      categories: cats,
      abs_url: full(absHref),
      pdf_url: pdfA ? full(pdfA.getAttribute('href')) : '',
    });
  }
  return { count: results.length, results: results, title: document.title || '' };
})()
