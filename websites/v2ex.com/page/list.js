// V2EX topic list: one record per `div.cell.item`.
(() => {
  const OPT = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const limit = (OPT.limit | 0) || 30;
  const clean = (s) => (s || '').replace(/[\s\u00a0]+/g, ' ').trim();
  const full = (href) => { try { return new URL(href, location.href).href; } catch (e) { return href || ''; } };
  const results = [];
  // Home tabs use `div.cell.item`; node pages use `div.cell` — select both and
  // keep only the rows that actually carry a topic title.
  const items = document.querySelectorAll('div.cell');
  for (let i = 0; i < items.length && results.length < limit; i++) {
    const it = items[i];
    const a = it.querySelector('.item_title a.topic-link');
    if (!a) continue;
    const href = a.getAttribute('href') || '';
    const idm = href.match(/\/t\/(\d+)/);
    const node = it.querySelector('a.node');
    const info = it.querySelector('.topic_info');
    let author = '', time = '', last = '';
    if (info) {
      const strongs = info.querySelectorAll('strong a');
      if (strongs.length > 0) author = clean(strongs[0].textContent);
      if (strongs.length > 1) last = clean(strongs[strongs.length - 1].textContent);
      const tm = info.querySelector('span[title]') || info.querySelector('span[data-original-title]');
      if (tm) time = tm.getAttribute('title') || tm.getAttribute('data-original-title') || clean(tm.textContent);
    }
    const cnt = it.querySelector('a.count_livid') || it.querySelector('.votes');
    const replies = cnt ? (parseInt(clean(cnt.textContent).replace(/[^0-9]/g, ''), 10) || 0) : 0;
    results.push({
      rank: results.length + 1,
      id: idm ? idm[1] : '',
      title: clean(a.textContent),
      url: full(href),
      node: node ? clean(node.textContent) : '',
      node_url: node ? full(node.getAttribute('href')) : '',
      author: author,
      time: time,
      last_reply_by: last,
      replies: replies,
    });
  }
  return { count: results.length, results: results, page_title: clean(document.title) };
})()
