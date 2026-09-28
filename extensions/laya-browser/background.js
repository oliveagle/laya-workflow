chrome.action.onClicked.addListener(async (tab) => {
  if (!tab.id) return;
  try {
    await chrome.scripting.executeScript({
      target: { tabId: tab.id },
      world: "MAIN",
      func: () => window.__layaBrowser?.highlight(!document.getElementById("__laya-browser-overlays"))
    });
  } catch (error) {
    console.warn("Laya Browser Bridge cannot access this page:", error);
  }
});
