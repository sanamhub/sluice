// Tells each .glow element where the pointer is over it, so its border can light up there. One
// listener for the page, and nothing at all on a device with no hover.
if (matchMedia("(hover: hover)").matches) {
  document.addEventListener(
    "pointermove",
    (event) => {
      const card = (event.target as Element | null)?.closest<HTMLElement>(
        ".glow",
      );
      if (!card) return;
      const box = card.getBoundingClientRect();
      card.style.setProperty("--mx", `${event.clientX - box.left}px`);
      card.style.setProperty("--my", `${event.clientY - box.top}px`);
    },
    { passive: true },
  );
}

export {};
