// Turn Wikipedia into the article alone: drop site chrome and maintenance or
// series boxes, and put the page title back at the top of the body so the
// rendered Markdown starts with an `# H1`.
(() => {
  const drop = [
    '.mw-editsection', '.mw-empty-elt', '.navbox', '.vertical-navbox',
    '.sidebar', '.ambox', '.mbox-small', '.mbox-text', '.mbox-image',
    '.tmbox', '.cmbox', '.ombox', '.fmbox', '.mw-authority-control',
    '.printfooter', '.mw-jump-link', '.catlinks', '.noprint',
    'style', 'link',
  ];
  let removed = 0;
  for (const sel of drop) {
    for (const el of document.querySelectorAll(sel)) { el.remove(); removed++; }
  }

  const title = ((document.querySelector('#firstHeading') || {}).innerText || '').trim();
  const body = document.querySelector('#mw-content-text');
  if (title && body && !body.querySelector('h1')) {
    const h = document.createElement('h1');
    h.textContent = title;
    body.insertBefore(h, body.firstChild);
  }
  return { removed: removed, title: title };
})()
