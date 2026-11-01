// @ts-check
// What the Install section shows. Until the first release is published, nothing on the page may
// point at the install script or /releases/latest (both would 404 or fail); check-links.mjs
// enforces that on the built site.

/**
 * @typedef {{
 *  defaultTab: 'script'|'cargo',
 *  script: {command: string|null, copy: boolean, note: string},
 *  cargo: {command: string, note: string},
 *  binary: {href: string, label: string, note: string},
 * }} InstallView
 */

/**
 * @param {{releaseAvailable: boolean, releasesUrl: string, latestReleaseUrl: string, commands: {script: string, cargo: string}}} site
 * @returns {InstallView}
 */
export function installView(site) {
 const cargo = { command: site.commands.cargo, note: 'Builds from source with a Rust toolchain.' };
 if (!site.releaseAvailable) return {
  defaultTab: 'cargo',
  script: { command: null, copy: false, note: 'The install script ships with the first release (v0.1.0). Use Cargo until then.' },
  cargo,
  binary: { href: site.releasesUrl, label: 'GitHub Releases', note: 'Binaries arrive with the v0.1.0 release.' },
 };
 return {
  defaultTab: 'script',
  script: { command: site.commands.script, copy: true, note: 'Installs the latest release to ~/.local/bin. Add --version X.Y.Z or --prefix DIR after sh -s --.' },
  cargo,
  binary: { href: site.latestReleaseUrl, label: 'Download the latest release', note: 'Archives for macOS (arm64, x86_64) and Linux (x86_64, arm64), with SHA-256 checksums.' },
 };
}

/**
 * The command of the tab the Install section opens on; the closing CTA band repeats it.
 * @param {InstallView} view
 * @returns {{command: string, copy: boolean}}
 */
export function defaultCommand(view) {
 if (view.defaultTab === 'cargo' || view.script.command === null) return { command: view.cargo.command, copy: true };
 return { command: view.script.command, copy: view.script.copy };
}
