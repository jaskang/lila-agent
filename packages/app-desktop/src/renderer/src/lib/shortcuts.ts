const SHORTCUT_OVERLAYS = [
  "dialog-content",
  "alert-dialog-content",
  "sheet-content",
  "popover-content",
  "select-content",
  "combobox-content",
  "dropdown-menu-content",
].map((slot) => `[data-slot="${slot}"]:not([data-closed])`).concat("[data-composer-suggest]").join(", ");

export function hasOpenShortcutOverlay(): boolean {
  for (const element of document.querySelectorAll(SHORTCUT_OVERLAYS)) {
    if (element.getClientRects().length && getComputedStyle(element).visibility === "visible") return true;
  }
  return false;
}
