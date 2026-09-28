(() => {
  "use strict";
  if (window.__layaBrowser) return;

  const MAX_ELEMENTS = 300;
  const STYLE_ID = "__laya-browser-style";
  const OVERLAY_ID = "__laya-browser-overlays";
  const DATA_INDEX = "data-laya-index";
  const state = { observation: 0 };

  const style = document.createElement("style");
  style.id = STYLE_ID;
  style.textContent = `
    [${DATA_INDEX}] { outline: 1px solid rgba(56,189,248,.55); }
    .__laya-badge {
      position: absolute; z-index: 2147483000; min-width: 16px; height: 16px;
      padding: 0 3px; border-radius: 7px; background: #0f172a; color: #e0f2fe;
      font: 600 10px/16px system-ui, sans-serif; text-align: center;
      box-shadow: 0 0 0 1px rgba(224,242,254,.5); pointer-events: none;
    }`;
  document.documentElement.appendChild(style);

  function text(el, limit = 300) {
    return String(
      el.getAttribute("aria-label") ||
        el.innerText ||
        el.value ||
        el.getAttribute("placeholder") ||
        el.name ||
        el.id ||
        ""
    ).replace(/\s+/g, " ").trim().slice(0, limit);
  }

  function visible(el) {
    if (!el.isConnected) return false;
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return false;
    const s = getComputedStyle(el);
    return s.display !== "none" && s.visibility !== "hidden" && s.pointerEvents !== "none";
  }

  function role(el) {
    return String(el.getAttribute("role") || "").trim() ||
      ({ A: "link", BUTTON: "button", INPUT: el.type === "checkbox" ? "checkbox" : "textbox",
         SELECT: "select", TEXTAREA: "textbox", SUMMARY: "summary" }[el.tagName] ||
        el.tagName.toLowerCase());
  }

  function section(el) {
    const heading = el.closest("h1,h2,h3,h4,h5,h6,[role=heading]");
    return heading ? text(heading, 120) : null;
  }

  function clearBadges() {
    document.getElementById(OVERLAY_ID)?.remove();
    document.querySelectorAll(`[${DATA_INDEX}]`).forEach((el) => el.removeAttribute(DATA_INDEX));
  }

  function clearObservation() {
    state.observation++;
    clearBadges();
  }

  function snapshot(maxText = 6000, annotate = true) {
    const candidates = [...document.querySelectorAll(
      "a[href],button,input:not([type=hidden]),select,textarea,summary,[role=button],[role=link],[role=checkbox],[role=combobox],[role=tab],[onclick]"
    )].filter(visible).slice(0, MAX_ELEMENTS);
    if (annotate) clearBadges();
    const elements = candidates.map((el, index) => {
      if (annotate) el.setAttribute(DATA_INDEX, String(index));
      const r = el.getBoundingClientRect();
      return {
        index,
        role: role(el),
        name: text(el),
        value: el.value ?? null,
        href: el instanceof HTMLAnchorElement ? el.href : null,
        section: section(el),
        disabled: Boolean(el.disabled) || el.getAttribute("aria-disabled") === "true",
        required: Boolean(el.required) || el.getAttribute("aria-required") === "true",
        rect: { x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height) },
        selector: el.id ? `#${CSS.escape(el.id)}` : null
      };
    });
    const bodyText = document.body?.innerText || document.documentElement.innerText || "";
    return {
      version: state.observation,
      url: location.href,
      title: document.title,
      text: bodyText.replace(/\n{3,}/g, "\n\n").slice(0, maxText),
      elements
    };
  }

  function element(index) {
    return document.querySelector(`[${DATA_INDEX}="${Number(index)}"]`);
  }

  function highlight(enabled = true) {
    if (!enabled) { clearBadges(); return false; }
    const snap = snapshot(0, true);
    const layer = document.createElement("div");
    layer.id = OVERLAY_ID;
    for (const item of snap.elements) {
      const el = element(item.index);
      if (!el) continue;
      const r = el.getBoundingClientRect();
      const badge = document.createElement("div");
      badge.className = "__laya-badge";
      badge.textContent = String(item.index);
      badge.style.left = `${window.scrollX + Math.max(0, r.left - 6)}px`;
      badge.style.top = `${window.scrollY + Math.max(0, r.top - 18)}px`;
      layer.appendChild(badge);
    }
    document.documentElement.appendChild(layer);
    return snap.elements.length > 0;
  }

  window.__layaBrowser = { snapshot, element, highlight, clear: clearObservation, version: 1 };
})();
