// The window's theme, before the first paint: the person's choice (Settings, Appearance) —
// "light" or "dark" on <html> — or none, macOS' own setting. app.js changes it.
'use strict';
(() => {
  let choice = null;
  try {
    choice = localStorage.getItem('wysebee-theme');
  } catch {
    // No storage: macOS' own setting.
  }
  if (choice === 'light' || choice === 'dark') document.documentElement.dataset.theme = choice;
})();
