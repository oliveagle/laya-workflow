// wall.js — is the current page behind an anti-bot wall?
//
// Taobao answers a client it distrusts with a challenge, and there are several
// shapes of it:
//   * the `J_MIDDLEWARE_FRAME_WIDGET` overlay (a 滑块 / click-the-image / drag-drop
//     captcha in an iframe);
//   * the Alibaba `baxia` shell — a *fixed, full-viewport* dialog
//     (`.baxia-dialog`, opaque `.baxia-dialog-mask`) whose content is an iframe at
//     `.../_____tmd_____/punish?x5secdata=…&action=captchadrag`, i.e. the same
//     滑块 the user sees ("亲，请拖动下方滑块完成验证");
//   * a whole-page "验证码拦截" / "访问太频繁".
//
// The last two are why the URL check alone is not enough: the top frame stays at
// the friendly `www.taobao.com/` while the challenge lives in a child iframe, so
// the address bar never mentions `punish`. Read the frames, not just the location.
(function () {
  "use strict";
  var title = document.title || "";
  var href = location.href || "";

  function visible(sel) {
    var e = document.querySelector(sel);
    if (!e) return null;
    var s = getComputedStyle(e);
    if (s.display === "none" || s.visibility === "hidden") return null;
    var r = e.getBoundingClientRect();
    if (r.width < 40 || r.height < 40) return null;
    return e;
  }

  var frames = document.querySelectorAll("iframe[src]");
  var punished = false;
  for (var i = 0; i < frames.length; i++) {
    if (/punish|x5sec|captchadrag|_____tmd_____/i.test(frames[i].getAttribute("src") || "")) {
      punished = true;
      break;
    }
  }

  var shell = !!(visible(".J_MIDDLEWARE_FRAME_WIDGET")
    || visible(".baxia-dialog-content")
    || visible(".baxia-dialog-mask"));

  return {
    wall: shell || punished
      || /验证码|拦截|访问过(于)?频繁|访问太频繁/.test(title)
      || /punish|x5sec|captcha|_____tmd_____/i.test(href),
    shell: shell,
    punished: punished,
    url: href,
    title: title,
    ready: document.readyState
  };
})()
