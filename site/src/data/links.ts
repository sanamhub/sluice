// Where the things the page links to live. The docs stay in the repository: this site is the
// landing page, and a link is better than a copy that drifts.
export const REPO = "https://github.com/sanamhub/sluice";
export const RELEASES = `${REPO}/releases`;
export const BLOB = `${REPO}/blob/main`;
export const SPEC = `${BLOB}/SPEC.md`;
export const THREAT_MODEL = `${BLOB}/docs/threat-model.md`;
export const DESIGN = `${BLOB}/docs/design.md`;
export const ADRS = `${REPO}/tree/main/docs/adr`;
export const PERFORMANCE = `${BLOB}/docs/performance.md`;
export const ROADMAP = `${BLOB}/docs/roadmap.md`;
export const INSTALL = `${BLOB}/docs/install.md`;
export const CHANGELOG = `${BLOB}/CHANGELOG.md`;
export const CLI_README = `${BLOB}/crates/sluice-cli/README.md`;
export const RELAY_README = `${BLOB}/docker/relay/README.md`;
export const CLI_IMAGE_README = `${BLOB}/docker/cli/README.md`;
export const CONTRIBUTING = `${BLOB}/CONTRIBUTING.md`;
export const SECURITY = `${BLOB}/SECURITY.md`;
export const LICENSES = `${REPO}#license`;
export const IROH = "https://github.com/n0-computer/iroh";
export const IROH_BLOBS = "https://github.com/n0-computer/iroh-blobs";

/** A section of the install guide, by its heading. */
export const installAt = (anchor: string) => `${INSTALL}#${anchor}`;
