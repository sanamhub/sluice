// What the landing page previews under /v/ share: the list of versions, the install options, the
// recorded run and the 0.2.0 work. One copy, so the three pages cannot disagree on a fact.
import { BREW, POWERSHELL_BY_HAND, SCOOP, SHELL } from "./install";

export const VERSIONS = [
  {
    id: 1,
    href: "/v/1/",
    name: "Speed",
    idea: "The path a file takes, a replay of a real run, and the numbers that are measured beside the ones that are still targets.",
    for: "People who pick a transfer tool by how it moves bytes.",
  },
  {
    id: 2,
    href: "/v/2/",
    name: "Everyone",
    idea: "One action, three steps, and plain answers. The ticket is the picture: the one thing that goes from you to them.",
    for: "People who have a big file and want it on another computer.",
  },
  {
    id: 3,
    href: "/v/3/",
    name: "Agents",
    idea: "The same transfer driven three ways: typed by a person, run by a script, called by an agent.",
    for: "People who automate, and the agents they run.",
  },
] as const;

export const QUICK_INSTALL = [
  { id: "unix", label: "macOS and Linux", commands: [{ lines: SHELL }] },
  {
    id: "windows",
    label: "Windows",
    commands: [{ lines: SCOOP }, POWERSHELL_BY_HAND],
  },
  { id: "brew", label: "Homebrew", commands: [{ lines: BREW }] },
];

export interface Line {
  kind: "cmd" | "out" | "err" | "ok";
  text: string;
}

// A real run, copied from the terminal: a folder of two videos and a note sent between two homes
// on one machine with the 0.1.0 binary. The ticket is shortened and the output path is written as
// a relative one; every other character is what the commands printed.
export const RUN_SENDER: Line[] = [
  { kind: "cmd", text: "sluice send holiday-video" },
  { kind: "out", text: "sluice10406jfphae599kc7nksv9zf46r051bq578p1jtv6…" },
  { kind: "err", text: "sending 3 files (75.3 MiB) with cubic" },
  { kind: "err", text: "receiver pkj66k91xk connected directly" },
  {
    kind: "ok",
    text: "receiver pkj66k91xk finished, the whole transfer (directly)",
  },
];

export const RUN_RECEIVER: Line[] = [
  { kind: "cmd", text: "sluice recv sluice10406jfph… --out ./out" },
  { kind: "err", text: "connected direct" },
  { kind: "err", text: "3 files, 1 folder, 75.3 MiB" },
  { kind: "err", text: "receiving 3 files (75.3 MiB) with cubic" },
  { kind: "err", text: "60% 45.8 MiB of 75.3 MiB" },
  { kind: "ok", text: "verified 75.3 MiB" },
  { kind: "ok", text: "done, 75.3 MiB in 0.9 s" },
  { kind: "err", text: "wrote 3 files under ./out" },
];

export const RUN_NOTE =
  "A real run between two homes on one machine, 0.1.0 on Windows. The ticket is shortened and the output path made relative.";

// The 0.2.0 work. None of it is in a release yet, and every page says so beside it.
export const COMING = [
  {
    id: "receipts",
    title: "Delivery receipts",
    plain: "Proof that the other side got the whole file.",
    body: "A receiver can ask the sender for a signed receipt, and anyone can check one offline.",
    code: "sluice receipt verify",
  },
  {
    id: "delta",
    title: "Delta receives",
    plain: "Send a new version and move only what changed.",
    body: "A receive names an older copy and fetches only the parts that differ. The result is still verified against the BLAKE3 root.",
    code: "sluice recv <ticket> --delta-from ./old-copy",
  },
  {
    id: "daemon",
    title: "A local daemon",
    plain: "Sluice running in the background, ready for other programs.",
    body: "It serves a control interface over gRPC on a socket or pipe that only your user can reach.",
    code: "sluice daemon",
  },
  {
    id: "mcp",
    title: "A server for AI agents",
    plain: "An AI assistant can send and receive for you.",
    body: "A Model Context Protocol server over standard input and output, with tools for send, receive, inspect, jobs and doctor.",
    code: "sluice mcp",
  },
  {
    id: "ui",
    title: "A page in your browser",
    plain: "Send and receive by clicking, with no terminal.",
    body: "The daemon serves a page on 127.0.0.1 only. The page asks the daemon to transfer and never carries file bytes itself.",
    code: "sluice ui",
  },
] as const;
