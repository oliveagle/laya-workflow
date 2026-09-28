# Laya Browser Bridge

Load this folder through Laya's singleton launcher (`launch: true`) or manually via
`chrome://extensions` → **Load unpacked**. The content script runs in the page main
world and exposes:

```js
__layaBrowser.snapshot(maxText = 6000) // URL/title/visible text + interactive element table
__layaBrowser.element(index)           // DOM node for the snapshot's stable index
__layaBrowser.highlight(true|false)    // numbered badges, also toggled from the toolbar button
```

The extension is the observation layer. Laya executes clicks and keystrokes with real
Chrome DevTools Protocol `Input.*` events, so there are never two automation browsers.
