// Counts each [data-count] number up from zero the first time it scrolls into view. The final
// number is already in the page, so with no script or with reduced motion it just reads.
const calm = matchMedia("(prefers-reduced-motion: reduce)").matches;

if (!calm && "IntersectionObserver" in window) {
  const seen = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        seen.unobserve(entry.target);
        const el = entry.target as HTMLElement;
        const to = Number(el.dataset.count);
        const places = (el.dataset.count ?? "").split(".")[1]?.length ?? 0;
        const start = performance.now();
        const step = (now: number) => {
          const t = Math.min(1, (now - start) / 900);
          const eased = 1 - (1 - t) ** 3;
          el.textContent = (to * eased).toFixed(places);
          if (t < 1) requestAnimationFrame(step);
        };
        requestAnimationFrame(step);
      }
    },
    { threshold: 0.6 },
  );
  for (const el of document.querySelectorAll<HTMLElement>("[data-count]"))
    seen.observe(el);
}

export {};
