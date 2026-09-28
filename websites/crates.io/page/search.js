// crates.io search results: one record per `div.crate-row`. crates.io is a
// Svelte SPA that paints its rows after load, so wait for them first.
(async () => {
  const OPT = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const limit = (OPT.limit | 0) || 10;
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
  const full = (href) => { try { return new URL(href, location.href).href; } catch (e) { return href || ''; } };
  await until('div.crate-row', 25000);
  const results = [];
  const rows = document.querySelectorAll('div.crate-row');
  for (let i = 0; i < rows.length && results.length < limit; i++) {
    const row = rows[i];
    const a = row.querySelector('a.name[href^="/crates/"]');
    if (!a) continue;
    const ver = row.querySelector('.version');
    const desc = row.querySelector('.description');
    let downloads = 0;
    const dl = row.querySelector('.downloads span span:last-child, .downloads');
    if (dl) downloads = parseInt(clean(dl.textContent).replace(/[^0-9]/g, ''), 10) || 0;
    const name = clean(a.textContent);
    results.push({
      rank: results.length + 1,
      name: name,
      version: ver ? clean(ver.textContent).replace(/^v/, '') : '',
      description: clean(desc ? desc.textContent : ''),
      downloads: downloads,
      url: full(a.getAttribute('href')),
      docs_url: 'https://docs.rs/' + name + '/latest/' + name.replace(/-/g, '_') + '/',
    });
  }
  return { count: results.length, results: results, page_title: clean(document.title) };
})()
