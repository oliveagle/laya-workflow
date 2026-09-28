// alphaXiv "AI Overview" section driver.
//
// A paper page carries its AI overview in `#overview`. Five states matter:
//   ready        — an overview exists; `#overview` holds the rendered article.
//   placeholder — none exists yet; the section shows a "Generate overview"
//                 (or "Try again") button, and a Markdown capture records
//                 "No overview yet…" as if it were the overview itself.
//   generating   — a request is in flight; the section shows a progress bar
//                 and the live channel swaps in the overview when it lands.
//   absent       — the section is not on the page (the SPA is mid-navigation,
//                 e.g. the sign-in round trip the click can trigger).
//   unknown      — the section is present but still a short skeleton.
//
// `__LAYA_OPTS__.action`:
//   probe  → report the state once (used to decide whether to click).
//   click  → click the generate button, report what happened.
//   wait   → poll until the state reaches one of `until` (default: ready) or
//            the per-call budget (`timeout_ms`) runs out. It deliberately does
//            *not* stop on `placeholder`: right after a click the button can
//            still be there for a beat, and returning early would spin.
//   text   → return the section's innerText, so the caller can size the result.
//
// Every branch resolves with `{state, len, buttons, href, clicked, waited_ms}`.
// `state` is one of: ready | placeholder | generating | absent | unknown.
(() => {
  const OPTS = (typeof __LAYA_OPTS__ !== 'undefined' && __LAYA_OPTS__) || {};
  const action = OPTS.action || 'probe';
  const SECTION_ID = OPTS.section_id || 'overview';
  // The rendered overview is thousands of chars; the placeholder is ~110 and
  // the progress bar ~150. 400 separates them without depending on the copy.
  const READY_MIN = OPTS.ready_min || 400;
  const POLL_MS = OPTS.poll_ms || 3000;
  const UNTIL = Array.isArray(OPTS.until) && OPTS.until.length ? OPTS.until : ['ready'];
  const deadline = Date.now() + ((OPTS.timeout_ms | 0) || 5000);
  const CTA = /generate overview|try again|重新生成|生成概览/i;
  // Progress copy from PublishPaperDialog: the request is queued, then the
  // agent reads the paper, writes, and checks citations.
  const BUSY = /queued for processing|reading the paper|writing the overview|checking citations|generation takes about|generating your new overview/i;

  const section = () => document.getElementById(SECTION_ID);
  const cta = () => {
    const ov = section();
    if (!ov) return null;
    return Array.from(ov.querySelectorAll('button, a[role="button"]'))
      .find((b) => CTA.test((b.textContent || '').trim())) || null;
  };
  const buttons = () => {
    const ov = section();
    if (!ov) return [];
    return Array.from(ov.querySelectorAll('button, a[role="button"]'))
      .map((b) => (b.textContent || '').trim())
      .filter(Boolean);
  };

  const read = () => {
    const ov = section();
    const txt = ov ? (ov.innerText || '') : '';
    const state = (() => {
      if (!ov) return 'absent';
      if (cta()) return 'placeholder';
      if (BUSY.test(txt)) return 'generating';
      // The section renders before its content streams in; treat a short body
      // as "not ready yet" rather than as a finished (empty) overview.
      if (txt.replace(/AI OVERVIEW/i, '').trim().length < READY_MIN) return 'unknown';
      return 'ready';
    })();
    return {
      state,
      len: txt.length,
      buttons: buttons().slice(0, 12),
      href: location.href,
      clicked: false,
      waited_ms: 0,
    };
  };

  const finish = (extra) => Object.assign(read(), extra || {});

  if (action === 'text') {
    const ov = section();
    return Promise.resolve(finish({ text: ov ? (ov.innerText || '') : '' }));
  }

  if (action === 'click') {
    const b = cta();
    if (!b) return Promise.resolve(finish({ clicked: false }));
    b.click();
    return Promise.resolve(finish({ clicked: true }));
  }

  if (action === 'wait') {
    const started = Date.now();
    return new Promise((resolve) => {
      const tick = () => {
        const now = finish({ waited_ms: Date.now() - started });
        if (UNTIL.indexOf(now.state) >= 0 || Date.now() > deadline) {
          resolve(now);
        } else {
          setTimeout(tick, POLL_MS);
        }
      };
      tick();
    });
  }

  return Promise.resolve(finish());
})()
