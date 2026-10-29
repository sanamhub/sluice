// Reads the page's colour tokens as the browser resolved them, so the scene matches the theme. A
// custom property holding light-dark() is not resolved by getComputedStyle, but a colour that uses
// it is, so each token is read through a probe element's `color`.
import { Color } from "three";

export const TOKENS = [
  "basin",
  "stone",
  "stone-2",
  "line",
  "line-strong",
  "mist",
  "water",
  "water-deep",
  "brass",
  "ink",
] as const;
export type Token = (typeof TOKENS)[number];
export type Palette = Record<Token, Color>;

export function readPalette(host: HTMLElement): Palette {
  const probe = document.createElement("span");
  probe.hidden = true;
  host.append(probe);
  const palette = {} as Palette;
  for (const token of TOKENS) {
    probe.style.color = `var(--${token})`;
    palette[token] = new Color(getComputedStyle(probe).color);
  }
  probe.remove();
  return palette;
}

/** True when the page is drawn in its light theme. */
export function isLight(): boolean {
  return getComputedStyle(document.documentElement).colorScheme.includes(
    "light",
  );
}
