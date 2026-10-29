// The header's theme button. It flips between light and dark, remembers the choice, and tells the
// hero scene through the attribute it already watches.
const KEY = "sluice-theme";

function current(): "light" | "dark" {
  const set = document.documentElement.dataset.theme;
  if (set === "light" || set === "dark") return set;
  return matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

function label(button: HTMLButtonElement) {
  const next = current() === "dark" ? "light" : "dark";
  button.setAttribute("aria-label", `Switch to the ${next} theme`);
  button.dataset.mode = current();
}

for (const button of document.querySelectorAll<HTMLButtonElement>(
  "[data-theme-toggle]",
)) {
  label(button);
  button.hidden = false;
  button.addEventListener("click", () => {
    const next = current() === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem(KEY, next);
    } catch {
      // A blocked store keeps the choice for this page only.
    }
    label(button);
  });
}
