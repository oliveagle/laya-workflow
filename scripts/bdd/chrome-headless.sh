#!/bin/sh
# exec a real Chrome with headless flags prepended, then pass through whatever
# laya-workflow's `chrome_cdp` capability appends (--remote-debugging-port,
# --user-data-dir, --enable-unsafe-extension-debugging, about:blank, ...).
#
# Why this exists instead of a `headless` field on BrowserCap: Chrome does not
# deliver synthetic Input.dispatchMouseEvent to a page whose visibilityState is
# "hidden", and a headful window nobody is looking at reports exactly that. The
# CDP call still returns success, so a scripted click looks like it worked while
# the page never saw it - which makes any BDD scenario that clicks a test of
# nothing. Headless fixes it; this is the least invasive way to get it without
# touching the capability's launch path.
#
# `exec` matters twice over: the pid stays the same (so the capability's owner
# marker and process-command-line check still identify this instance), and no
# wrapper process lingers holding the profile lock.
#
# The real binary comes from $LAYA_BDD_REAL_CHROME (the runner resolves it the
# same way the engine does), else $CHROME_BIN, else the platform default.
set -eu

real="${LAYA_BDD_REAL_CHROME:-${CHROME_BIN:-}}"
if [ -z "$real" ]; then
  for candidate in \
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
    "/Applications/Chromium.app/Contents/MacOS/Chromium" \
    "/usr/bin/google-chrome" \
    "/usr/bin/google-chrome-stable" \
    "/usr/bin/chromium" \
    "/usr/bin/chromium-browser"
  do
    if [ -f "$candidate" ]; then real="$candidate"; break; fi
  done
fi
if [ -z "$real" ] || [ ! -x "$real" ]; then
  echo "chrome-headless.sh: no Chrome binary found (set LAYA_BDD_REAL_CHROME or CHROME_BIN)" >&2
  exit 1
fi

# --window-size: the default headless viewport is small enough that elements
# land off-screen and have to be scrolled into; pin a realistic one.
exec "$real" \
  --headless=new \
  --disable-gpu \
  --hide-scrollbars \
  --window-size=1280,900 \
  "$@"
