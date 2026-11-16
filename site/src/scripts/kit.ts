// The motion kit of previews 4 to 6: eased scrolling, the pull of a button toward the pointer,
// headlines that rise word by word, and cards that lean. None of it runs with reduced motion, and
// the pointer effects run only where there is a pointer that hovers.
import Lenis from "lenis";

const calm = matchMedia("(prefers-reduced-motion: reduce)").matches;
const hovers = matchMedia("(hover: hover) and (pointer: fine)").matches;

if (!calm) {
  // Wheel and keyboard scrolling ease to a stop instead of stepping. Touch scrolling is left to
  // the device, which already does this.
  new Lenis({ autoRaf: true, anchors: true, lerp: 0.11 });

  // Each word of a [data-split] headline gets its own clipped box and rises when the headline
  // comes into view. The words stay ordinary text in the same order.
  const seen = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        seen.unobserve(entry.target);
        entry.target.classList.add("in");
      }
    },
    { threshold: 0.35 },
  );
  for (const el of document.querySelectorAll<HTMLElement>("[data-split]")) {
    let at = 0;
    const walk = (node: Node) => {
      for (const child of [...node.childNodes]) {
        if (child.nodeType === Node.TEXT_NODE) {
          const parts = (child.textContent ?? "").split(/(\s+)/);
          const made = document.createDocumentFragment();
          for (const part of parts) {
            if (part.trim() === "") {
              made.append(part);
              continue;
            }
            const box = document.createElement("span");
            box.className = "w";
            const word = document.createElement("span");
            word.style.setProperty("--i", String(at++));
            word.textContent = part;
            box.append(word);
            made.append(box);
          }
          child.replaceWith(made);
        } else if (child.nodeType === Node.ELEMENT_NODE) walk(child);
      }
    };
    walk(el);
    el.classList.add("split-ready");
    seen.observe(el);
  }
}

if (!calm && hovers) {
  // A button moves a little toward the pointer while the pointer is near it.
  for (const el of document.querySelectorAll<HTMLElement>("[data-magnet]")) {
    el.addEventListener(
      "pointermove",
      (event) => {
        const box = el.getBoundingClientRect();
        const x = event.clientX - (box.left + box.width / 2);
        const y = event.clientY - (box.top + box.height / 2);
        el.style.setProperty("--tx", `${(x * 0.22).toFixed(1)}px`);
        el.style.setProperty("--ty", `${(y * 0.3).toFixed(1)}px`);
      },
      { passive: true },
    );
    el.addEventListener("pointerleave", () => {
      el.style.removeProperty("--tx");
      el.style.removeProperty("--ty");
    });
  }

  // A card leans a few degrees toward the pointer.
  for (const el of document.querySelectorAll<HTMLElement>("[data-tilt]")) {
    const most = Number(el.dataset.tilt) || 6;
    el.addEventListener(
      "pointermove",
      (event) => {
        const box = el.getBoundingClientRect();
        const x = (event.clientX - box.left) / box.width - 0.5;
        const y = (event.clientY - box.top) / box.height - 0.5;
        el.style.setProperty("--ry", `${(x * most).toFixed(2)}deg`);
        el.style.setProperty("--rx", `${(-y * most).toFixed(2)}deg`);
      },
      { passive: true },
    );
    el.addEventListener("pointerleave", () => {
      el.style.removeProperty("--rx");
      el.style.removeProperty("--ry");
    });
  }
}

export {};
