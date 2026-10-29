// Turns each [data-tabs] block into a tab list with arrow-key movement, and gives each command a
// copy button. Without this script every panel stays visible, with its own label.

function select(
  tabs: HTMLButtonElement[],
  panels: HTMLElement[],
  index: number,
  focus: boolean,
) {
  tabs.forEach((tab, i) => {
    const on = i === index;
    tab.setAttribute("aria-selected", String(on));
    tab.tabIndex = on ? 0 : -1;
    panels[i].hidden = !on;
  });
  if (focus) tabs[index].focus();
}

for (const block of document.querySelectorAll<HTMLElement>("[data-tabs]")) {
  const list = block.querySelector<HTMLElement>("[role=tablist]");
  const tabs = [...block.querySelectorAll<HTMLButtonElement>("[role=tab]")];
  const panels = [...block.querySelectorAll<HTMLElement>("[role=tabpanel]")];
  if (!list || tabs.length !== panels.length) continue;
  list.hidden = false;
  block.dataset.enhanced = "true";
  select(tabs, panels, 0, false);
  tabs.forEach((tab, i) =>
    tab.addEventListener("click", () => select(tabs, panels, i, false)),
  );
  list.addEventListener("keydown", (event) => {
    const at = tabs.indexOf(document.activeElement as HTMLButtonElement);
    if (at < 0) return;
    const moves: Record<string, number> = {
      ArrowRight: (at + 1) % tabs.length,
      ArrowLeft: (at - 1 + tabs.length) % tabs.length,
      Home: 0,
      End: tabs.length - 1,
    };
    if (!(event.key in moves)) return;
    event.preventDefault();
    select(tabs, panels, moves[event.key], true);
  });
}

for (const button of document.querySelectorAll<HTMLButtonElement>(
  "[data-copy]",
)) {
  const code = button.parentElement?.querySelector("code");
  const text = button.querySelector(".copy-text");
  if (!code || !text || !navigator.clipboard) continue;
  button.hidden = false;
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(code.textContent ?? "");
      text.textContent = "Copied";
    } catch {
      text.textContent = "Select and copy";
    }
    setTimeout(() => (text.textContent = "Copy"), 1800);
  });
}
