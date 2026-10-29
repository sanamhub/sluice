// Runs before the first paint, from the head, so a chosen theme never flashes the other one.
// Nothing is set when nothing was chosen: the stylesheet then follows the system.
try {
  const theme = localStorage.getItem("sluice-theme");
  if (theme === "light" || theme === "dark")
    document.documentElement.dataset.theme = theme;
} catch {
  // Storage can be blocked; the system theme is the answer then.
}
