// docs.rs release search: one record per `a.release`.
(() => {
  const OPT = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const limit = (OPT.limit | 0) || 10;
  const clean = (s) => (s || '').replace(/[\s\u00a0]+/g, ' ').trim();
  const full = (href) => { try { return new URL(href, location.href).href; } catch (e) { return href || ''; } };
  const results = [];
  const items = document.querySelectorAll('a.release');
  for (let i = 0; i < items.length && results.length < limit; i++) {
    const a = items[i];
    const nameEl = a.querySelector('.name');
    const name = clean(nameEl ? nameEl.textContent : '');
    if (!name) continue;
    const desc = a.querySelector('.description');
    const date = a.querySelector('.date');
    const m = name.match(/^(.*)-(\d[\w.+-]*)$/);
    results.push({
      rank: results.length + 1,
      name: name,
      crate: m ? m[1] : name,
      version: m ? m[2] : '',
      description: clean(desc ? desc.textContent : ''),
      released: date ? (date.getAttribute('title') || clean(date.textContent)) : '',
      url: full(a.getAttribute('href')),
    });
  }
  return { count: results.length, results: results, page_title: clean(document.title) };
})()
