// PyPI project page: the name/version header (`<h1>`) as one record.
(() => {
  const clean = (s) => (s || '').replace(/[\s\u00a0]+/g, ' ').trim();
  const h1 = clean((document.querySelector('h1') || {}).innerText || '');
  const m = h1.match(/^(.*?)\s+([\d][\w.\-+]*)$/);
  const desc = document.querySelector('#description');
  return {
    name: m ? clean(m[1]) : h1,
    version: m ? m[2] : '',
    title: clean(document.title),
    description_chars: desc ? desc.innerText.length : 0,
  };
})()
