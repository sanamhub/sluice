// Every install command the site shows, in one place, so the hero and the install section cannot
// drift apart. Names match docs/install.md and the package files the release generates.

const DOWNLOAD = "https://github.com/sanamhub/sluice/releases/latest/download";

export const SHELL = [
  "curl --proto '=https' --tlsv1.2 -LsSf \\",
  `  ${DOWNLOAD}/sluice-cli-installer.sh | sh`,
];

// Never offered through the copy button: see InstallTabs.astro.
export const POWERSHELL = [`irm ${DOWNLOAD}/sluice-cli-installer.ps1 | iex`];

export const BREW = ["brew install sanamhub/tap/sluice"];

export const SCOOP = [
  "scoop bucket add sanamhub https://github.com/sanamhub/scoop-bucket",
  "scoop install sluice",
];

export const WINGET = ["winget install --id sanamhub.sluice -e"];

export const AUR = ["yay -S sluice-bin"];

export const DOCKER = ["docker pull ghcr.io/sanamhub/sluice:0.1.0"];

export const SOURCE = [
  "cargo install --git https://github.com/sanamhub/sluice sluice-cli --locked",
];

/** The PowerShell installer, shown under a package manager to be copied by hand. */
export const POWERSHELL_BY_HAND = {
  lines: POWERSHELL,
  copy: false,
  caption: "Or run the installer in PowerShell. Select the line to copy it.",
};
