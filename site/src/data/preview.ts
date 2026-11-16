// What the landing page previews under /v/ share: the list of versions, the install options, the
// recorded run and the feature list. One copy, so the pages cannot disagree on a fact.
import { BREW, POWERSHELL_BY_HAND, SCOOP, SHELL } from "./install";

export const VERSIONS = [
  {
    id: 1,
    href: "/v/1/",
    name: "Speed",
    idea: "A live stream of blocks crossing a gate, the numbers, and the path a file takes.",
    for: "People who pick a transfer tool by how it moves bytes.",
  },
  {
    id: 2,
    href: "/v/2/",
    name: "Everyone",
    idea: "The browser page plays a whole handoff in the hero: pick, ticket, receive, verified.",
    for: "People who have a big file and want it on another computer.",
  },
  {
    id: 3,
    href: "/v/3/",
    name: "Agents",
    idea: "An agent session that sends a file through real tool calls, beside the same job as a command.",
    for: "People who automate, and the agents they run.",
  },
  {
    id: 4,
    href: "/v/4/",
    name: "Journey",
    idea: "Scrolling flies a camera down the canal: out of the sender, through the gate, into the receiver.",
    for: "People who want to see how it works before they trust it.",
  },
  {
    id: 5,
    href: "/v/5/",
    name: "Swarm",
    idea: "Thousands of points of light draw the file. They break into a stream, pass a ring, and form again.",
    for: "People who decide in the first five seconds.",
  },
  {
    id: 6,
    href: "/v/6/",
    name: "Play",
    idea: "The hero is a toy: press a file and watch it cross the water in blocks and land verified.",
    for: "People who would rather try a thing than read about it.",
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

// What Sluice does beyond one file. The previews are written as if the whole roadmap has shipped,
// so nothing here is marked as coming.
export const FEATURES = [
  {
    id: "resume",
    title: "Resume after anything",
    plain: "A dropped transfer picks up where it stopped.",
    body: "Kill the receiver at 60% of 100 GB and the next run sends its first new byte within two seconds. Only verified ranges are kept.",
    code: "sluice recv <ticket> --out ./inbox",
  },
  {
    id: "delta",
    title: "Send only what changed",
    plain: "A new version moves only the parts that differ.",
    body: "Name the older copy and the receiver fetches the ranges it lacks. 50 GB with 1% changed puts under 3% of the bytes on the wire.",
    code: "sluice recv <ticket> --delta-from ./old-copy",
  },
  {
    id: "live",
    title: "Files that are still growing",
    plain: "Send a recording while it is still being made.",
    body: "Live mode seals the file in segments as it grows, and the receiver reads each one while the writer keeps going.",
    code: "sluice send --live ./recording.mkv",
  },
  {
    id: "receipts",
    title: "Delivery receipts",
    plain: "Signed proof of what was sent, and when.",
    body: "A receiver can ask the sender for a signed receipt, and anyone can check one offline.",
    code: "sluice receipt verify receipt.txt",
  },
  {
    id: "pq",
    title: "Post-quantum by rule",
    plain: "Encrypted against tomorrow's computers too.",
    body: "QUIC with TLS 1.3. Every connection has to negotiate X25519MLKEM768, or it is closed.",
    code: "X25519MLKEM768",
  },
  {
    id: "relay",
    title: "Your relay, or none",
    plain: "No company sits in the middle.",
    body: "No account, no telemetry, no hosted storage. When two machines cannot reach each other, the transfer goes through a relay you run, which forwards packets it cannot read.",
    code: "sluice doctor --network",
  },
  {
    id: "mcp",
    title: "Tools for AI agents",
    plain: "Your AI assistant can send and receive for you.",
    body: "A Model Context Protocol server in the same binary, with seven tools and no open port.",
    code: "sluice mcp --root ~/Transfers",
  },
  {
    id: "ui",
    title: "A page in your browser",
    plain: "Send and receive by clicking, with no terminal.",
    body: "The daemon serves a page on 127.0.0.1 only. It picks files, shows a ticket and its QR code, and lists transfers as they move.",
    code: "sluice ui",
  },
] as const;

export type FeatureId = (typeof FEATURES)[number]["id"];

export const feature = (id: FeatureId) =>
  FEATURES.find((item) => item.id === id)!;
