// One story page: header plus the top comments (depth from the indent image).
(() => {
  const OPT = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const limit = (OPT.limit | 0) || 20;
  const num = (s) => parseInt((s || '').replace(/[^0-9]/g, ''), 10) || 0;
  const clean = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const titleA = document.querySelector('.titleline > a') || document.querySelector('a.titlelink');
  const sub = document.querySelector('.subtext');
  let points = 0, user = '', age = '';
  if (sub) {
    const sc = sub.querySelector('.score');
    if (sc) points = num(sc.textContent);
    const u = sub.querySelector('.hnuser');
    if (u) user = clean(u.textContent);
    const ag = sub.querySelector('.age');
    if (ag) age = (ag.getAttribute('title') || ag.textContent || '').trim();
  }
  const textEl = document.querySelector('.toptext');
  const rows = document.querySelectorAll('tr.athing.comtr');
  const comments = [];
  for (let i = 0; i < rows.length && comments.length < limit; i++) {
    const tr = rows[i];
    const u = tr.querySelector('.hnuser');
    const c = tr.querySelector('.commtext');
    const ag = tr.querySelector('.age');
    const ind = tr.querySelector('.ind img');
    let depth = 0;
    if (ind) depth = Math.round(num(ind.getAttribute('width')) / 40);
    comments.push({
      depth: depth,
      user: u ? clean(u.textContent) : '',
      age: ag ? (ag.getAttribute('title') || ag.textContent || '').trim() : '',
      text: c ? (c.innerText || '').replace(/[ \t]+/g, ' ').replace(/\n{3,}/g, '\n\n').trim().slice(0, 4000) : '',
    });
  }
  return {
    title: titleA ? clean(titleA.textContent) : (document.title || ''),
    url: titleA ? (titleA.href || '') : '',
    points: points,
    user: user,
    age: age,
    story_text: textEl ? (textEl.innerText || '').trim() : '',
    comments_total: rows.length,
    comments: comments,
  };
})()
