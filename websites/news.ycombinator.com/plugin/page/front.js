// Front page (top / best / new / ask / show / jobs): one record per story.
(() => {
  const OPT = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const limit = (OPT.limit | 0) || 30;
  const num = (s) => parseInt((s || '').replace(/[^0-9]/g, ''), 10) || 0;
  const clean = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const stories = [];
  const rows = document.querySelectorAll('tr.athing');
  for (let i = 0; i < rows.length && stories.length < limit; i++) {
    const tr = rows[i];
    const id = tr.getAttribute('id') || '';
    const titleA = tr.querySelector('.titleline > a') || tr.querySelector('a.titlelink') || tr.querySelector('a[href]');
    const site = tr.querySelector('.sitestr');
    const next = tr.nextElementSibling;
    const sub = (next && next.tagName === 'TR') ? next : null;
    let points = 0, comments = 0, age = '', user = '', commentsUrl = '';
    if (sub) {
      const sc = sub.querySelector('.score');
      if (sc) points = num(sc.textContent);
      const u = sub.querySelector('.hnuser');
      if (u) user = clean(u.textContent);
      const ag = sub.querySelector('.age');
      if (ag) age = (ag.getAttribute('title') || ag.textContent || '').trim();
      const links = sub.querySelectorAll('a');
      for (let j = 0; j < links.length; j++) {
        const t = (links[j].textContent || '').trim();
        if (/comment/i.test(t)) { comments = num(t); commentsUrl = links[j].href || ''; }
      }
    }
    stories.push({
      id: id,
      rank: stories.length + 1,
      title: titleA ? clean(titleA.textContent) : '',
      url: titleA ? (titleA.href || '') : '',
      site: site ? clean(site.textContent) : '',
      points: points,
      comments: comments,
      user: user,
      age: age,
      hn_url: id ? ('https://news.ycombinator.com/item?id=' + id) : '',
      comments_url: commentsUrl,
    });
  }
  return { count: stories.length, page_title: document.title || '', stories: stories };
})()
