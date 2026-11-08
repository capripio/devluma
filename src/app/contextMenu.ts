/**
 * Turns off the WebView's right-click menu (Back, Reload, Inspect…), which belongs to a web
 * page, not a desktop app. Text fields keep it so Cut, Copy and Paste still work there.
 * Returns a function that turns it back on.
 */
export function disableContextMenu(target: Document = document): () => void {
  const handler = (e: MouseEvent) => {
    const el = e.target instanceof Element ? e.target : null;
    if (el?.closest("input, textarea, [contenteditable='true']")) return;
    e.preventDefault();
  };
  target.addEventListener("contextmenu", handler);
  return () => target.removeEventListener("contextmenu", handler);
}
