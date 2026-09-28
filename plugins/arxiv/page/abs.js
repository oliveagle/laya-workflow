// arXiv abstract page (`/abs/<id>`): a clean metadata digest.
(() => {
  const clean = (s) => (s || '').replace(/[\s\u00a0]+/g, ' ').trim();
  const strip = (s, p) => {
    const t = clean(s);
    return t.indexOf(p) === 0 ? t.slice(p.length).trim() : t;
  };
  const q = (sel) => document.querySelector(sel);
  const txt = (sel) => { const e = q(sel); return e ? clean(e.textContent) : ''; };
  const full = (href) => {
    if (!href) return '';
    if (/^https?:\/\//i.test(href)) return href;
    return 'https://arxiv.org' + (href.charAt(0) === '/' ? href : '/' + href);
  };
  const versioned = (location.pathname.match(/\/abs\/([^\/?#]+)/) || [])[1] || '';
  const base = versioned.replace(/v\d+$/i, '');
  const version = (versioned.match(/v\d+$/i) || [''])[0];
  const pdfA = q('a[href*="/pdf/"]');
  const htmlA = q('a[href*="/html/"]');
  const authors = [];
  const authorEls = document.querySelectorAll('#abs .authors a, .authors a');
  for (let i = 0; i < authorEls.length; i++) authors.push(clean(authorEls[i].textContent));
  return {
    arxiv_id: base,
    version: version,
    versioned_id: versioned,
    title: strip(txt('h1.title'), 'Title:'),
    authors_text: strip(txt('#abs .authors'), 'Authors:'),
    authors: authors,
    abstract: strip(txt('blockquote.abstract'), 'Abstract:'),
    dateline: txt('#abs .dateline').replace(/^\[|\]$/g, '').trim(),
    comments: txt('td.comments'),
    subjects: txt('td.subjects'),
    journal_ref: txt('td.journal-ref'),
    doi: txt('td.arxivdoi a'),
    abs_url: 'https://arxiv.org/abs/' + base,
    versioned_url: 'https://arxiv.org/abs/' + versioned,
    pdf_url: pdfA ? full(pdfA.getAttribute('href')) : ('https://arxiv.org/pdf/' + base),
    html_url: htmlA ? full(htmlA.getAttribute('href')) : '',
    page_title: clean(document.title),
  };
})()
