// wall.js — is the current page behind an anti-bot wall?
//
// Taobao answers a client it distrusts with a 滑块 / 验证码 overlay (the
// `J_MIDDLEWARE_FRAME_WIDGET` iframe, "亲，请拖动下方滑块完成验证") or, harder,
// a whole-page "验证码拦截" / "访问太频繁". Either way there is no product to
// read and no element to click, so `guard` reports it and waits.
(function () {
  "use strict";
  var title = document.title || "";
  var href = location.href || "";
  return {
    wall: !!document.querySelector(".J_MIDDLEWARE_FRAME_WIDGET")
      || /验证码|拦截|访问过(于)?频繁|访问太频繁/.test(title)
      || /punish|x5sec|captcha|_____tmd_____/i.test(href),
    url: href,
    title: title,
    ready: document.readyState
  };
})()
