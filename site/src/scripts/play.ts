// The toy in version 6's hero. Pressing a file on this computer's side breaks it into blocks; the
// blocks cross the water one after another, turn brass at the gate, and build the same file on the
// other side, marked verified. It is a drawing of a transfer, not one: nothing leaves the page.
//
// Without this script both sides show every file. The script empties the far side, so there is
// something to send. With reduced motion a file simply appears on the far side when pressed.
const pond = document.querySelector<HTMLElement>("[data-pond]");

if (pond) {
  const calm = matchMedia("(prefers-reduced-motion: reduce)");
  const files = [...pond.querySelectorAll<HTMLButtonElement>("[data-file]")];
  const landed = [...pond.querySelectorAll<HTMLElement>("[data-landed]")];
  const gate = pond.querySelector<HTMLElement>("[data-gate]")!;
  const empty = pond.querySelector<HTMLElement>("[data-empty]");
  const status = pond.querySelector<HTMLElement>("[data-status]");
  const again = pond.querySelector<HTMLButtonElement>("[data-again]");
  let touched = false;

  const count = () => landed.filter((item) => !item.hidden).length;

  function tell(text: string) {
    if (status) status.textContent = text;
  }

  function settle() {
    const n = count();
    if (empty) empty.hidden = n > 0;
    if (again) again.hidden = n < files.length;
  }

  function centre(el: Element) {
    const box = el.getBoundingClientRect();
    const home = pond!.getBoundingClientRect();
    return {
      x: box.left + box.width / 2 - home.left,
      y: box.top + box.height / 2 - home.top,
    };
  }

  function send(file: HTMLButtonElement) {
    if (file.disabled) return;
    const target = landed[files.indexOf(file)];
    const name = file.dataset.file ?? "";
    file.disabled = true;
    tell(`Sending ${name}.`);

    const arrive = () => {
      target.hidden = false;
      settle();
      tell(
        `${name} arrived and was verified. ${count()} of ${files.length} files are across.`,
      );
      if (!calm.matches)
        target.animate(
          [
            { scale: 0.9, opacity: 0 },
            { scale: 1.04, opacity: 1, offset: 0.6 },
            { scale: 1 },
          ],
          { duration: 420, easing: "cubic-bezier(.2,.9,.2,1)" },
        );
    };
    if (calm.matches) return arrive();

    // Measure where the file will land, without showing it yet.
    target.hidden = false;
    target.style.visibility = "hidden";
    if (empty) empty.hidden = true;
    const to = centre(target);
    target.hidden = true;
    target.style.visibility = "";
    const from = centre(file);
    const through = centre(gate);

    const blocks = 16;
    let done = 0;
    for (let n = 0; n < blocks; n++) {
      const block = document.createElement("i");
      block.className = "block";
      pond!.append(block);
      const spread = () => (Math.random() - 0.5) * 46;
      const flight = block.animate(
        [
          {
            translate: `${from.x + spread()}px ${from.y + spread() * 0.5}px`,
            scale: 0.4,
            opacity: 0,
          },
          { opacity: 1, scale: 1, offset: 0.12 },
          {
            translate: `${through.x}px ${through.y + (Math.random() - 0.5) * 18}px`,
            offset: 0.5,
          },
          { scale: 1.25, offset: 0.56 },
          { opacity: 1, scale: 1, offset: 0.9 },
          {
            translate: `${to.x + spread()}px ${to.y + spread() * 0.4}px`,
            scale: 0.4,
            opacity: 0,
          },
        ],
        {
          duration: 1150,
          delay: n * 55,
          easing: "cubic-bezier(.45,.05,.3,1)",
          fill: "both",
        },
      );
      // Brass from the gate on: the block passed its check.
      setTimeout(() => block.classList.add("through"), n * 55 + 1150 * 0.43);
      flight.finished.then(() => {
        block.remove();
        if (++done === blocks) arrive();
      });
    }
    gate.animate(
      [
        { opacity: 1, scale: "1 1" },
        { opacity: 1, scale: "2.4 1.08", offset: 0.5 },
        { opacity: 1, scale: "1 1" },
      ],
      {
        duration: 900,
        delay: 500,
        easing: "ease-out",
      },
    );
  }

  for (const item of landed) item.hidden = true;
  settle();
  pond.dataset.ready = "true";
  tell("Press a file to send it across.");

  for (const file of files)
    file.addEventListener("click", () => {
      touched = true;
      send(file);
    });

  again?.addEventListener("click", () => {
    for (const item of landed) item.hidden = true;
    for (const file of files) file.disabled = false;
    settle();
    tell("The other side is empty again. Press a file to send it across.");
    files[0].focus();
  });

  // Show what the toy does once, if the reader has not tried it by then.
  if (!calm.matches)
    setTimeout(() => {
      if (!touched && !document.hidden) send(files[0]);
    }, 2200);
}

export {};
