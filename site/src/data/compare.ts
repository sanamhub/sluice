// Sluice next to four tools people use for the same job. Every cell about another tool says only
// what that tool's own documentation says, and "Not stated" where it says nothing. The sources are
// listed under the table with the date they were read; check them again before changing a cell.

export const CHECKED = "30 October 2026";

export interface Tool {
  name: string;
  /** The page the cells were read from; shown under the table. */
  sources: { label: string; href: string }[];
}

export const TOOLS: Tool[] = [
  {
    name: "Sluice",
    sources: [],
  },
  {
    name: "croc",
    sources: [
      { label: "croc README", href: "https://github.com/schollz/croc" },
    ],
  },
  {
    name: "magic-wormhole",
    sources: [
      {
        label: "magic-wormhole docs",
        href: "https://magic-wormhole.readthedocs.io/en/latest/welcome.html",
      },
      {
        label: "file transfer protocol",
        href: "https://magic-wormhole.readthedocs.io/en/latest/file-transfer-protocol.html",
      },
    ],
  },
  {
    name: "LocalSend",
    sources: [
      { label: "localsend.org", href: "https://localsend.org/" },
      {
        label: "LocalSend protocol",
        href: "https://github.com/localsend/protocol",
      },
    ],
  },
  {
    name: "WeTransfer (free)",
    sources: [
      {
        label: "WeTransfer size limits",
        href: "https://wetransfer.com/resources/free-file-transfer/wetransfer-file-size-limit",
      },
      {
        label: "WeTransfer security",
        href: "https://wetransfer.com/help-center/security-privacy/how-do-we-protect-your-files",
      },
    ],
  },
];

const NOT_STATED = "Not stated";

/** One row: the question, then one answer per tool, in the order of TOOLS. */
export const ROWS: [string, ...string[]][] = [
  [
    "Where the file goes",
    "Straight to the other computer, or through a relay you run",
    "Direct, with a public relay as fallback; an optional mode stores it encrypted for a day",
    "Direct, with a public transit relay as fallback",
    "Straight to the other device, on the same local network",
    "Uploaded to and stored on WeTransfer's servers",
  ],
  ["Across the internet", "Yes", "Yes", "Yes", "No, local network only", "Yes"],
  ["Account", "None", NOT_STATED, NOT_STATED, "None", "The sender needs one"],
  [
    "Size limit",
    "None from Sluice",
    NOT_STATED,
    "None stated",
    NOT_STATED,
    "3 GB per transfer",
  ],
  [
    "What is checked",
    "Every block, against a BLAKE3 root, before it is written",
    "Each file is hashed",
    "Each record has a MAC; the receiver reports a SHA-256 of the file",
    "An optional SHA-256 of the whole file",
    NOT_STATED,
  ],
  [
    "After a dropped link",
    "Resumes, keeping the blocks it checked",
    "Resumes",
    "Both sides must stay running until it ends",
    NOT_STATED,
    NOT_STATED,
  ],
  [
    "Encryption",
    "End to end, QUIC with TLS 1.3",
    "End to end, keyed by a PAKE",
    "End to end, SPAKE2 and NaCl secretbox",
    "HTTPS between the two devices",
    "TLS in transit, AES-256 at rest",
  ],
  [
    "Run your own relay",
    "Yes, and it ships closed",
    "Yes",
    "Yes",
    "No relay",
    "No",
  ],
  [
    "Pipes and growing files",
    "Both",
    "Pipes",
    NOT_STATED,
    NOT_STATED,
    NOT_STATED,
  ],
  [
    "Licence",
    "Apache-2.0 or MIT",
    "MIT",
    "MIT",
    "Apache-2.0",
    "A hosted service",
  ],
];

export { NOT_STATED };
