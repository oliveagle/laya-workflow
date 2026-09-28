// A V2EX topic page: the opening post plus its replies.
(() => {
  const OPT = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const limit = (OPT.limit | 0) || 30;
  const clean = (s) => (s || '').replace(/[\s\u00a0]+/g, ' ').trim();
  const title = clean((document.querySelector('h1') || {}).innerText || '');
  const node = document.querySelector('a.node');
  const contentEl = document.querySelector('.topic_content');
  const replies = [];
  const items = document.querySelectorAll('div.cell[id^="r_"]');
  for (let i = 0; i < items.length && replies.length < limit; i++) {
    const r = items[i];
    const no = r.querySelector('.no');
    const a = r.querySelector('strong a.dark');
    const ago = r.querySelector('span.ago');
    const c = r.querySelector('.reply_content');
    replies.push({
      floor: no ? (parseInt(clean(no.textContent), 10) || 0) : 0,
      id: (r.id || '').replace(/^r_/, ''),
      user: a ? clean(a.textContent) : '',
      time: ago ? (ago.getAttribute('title') || '') : '',
      content: clean(c ? c.innerText : ''),
    });
  }
  return {
    title: title,
    node: node ? clean(node.textContent) : '',
    content: clean(contentEl ? contentEl.innerText : ''),
    replies: replies,
    count: replies.length,
    replies_total: replies.length,
    page_title: clean(document.title),
  };
})()
