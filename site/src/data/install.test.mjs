import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { installView } from './install.mjs';
import { site } from './site.ts';

const repo = 'https://github.com/nodera-studio/glen';
const cargo = { command: `cargo install --git ${repo} --locked`, note: 'Builds from source with a Rust toolchain.' };

test('before the first release: Cargo first, no installer, Releases page (AC-725)', () => {
 const view = installView({ ...site, releaseAvailable: false });
 assert.deepEqual(view, {
  defaultTab: 'cargo',
  script: { command: null, copy: false, note: 'The install script ships with the first release (v0.1.0). Use Cargo until then.' },
  cargo,
  binary: { href: `${repo}/releases`, label: 'GitHub Releases', note: 'Binaries arrive with the v0.1.0 release.' },
 });
 assert.doesNotMatch(JSON.stringify(view), /raw\.githubusercontent\.com|\/releases\/latest/);
});

test('after the release: the script with copy, the latest release (AC-725)', () => {
 assert.deepEqual(installView({ ...site, releaseAvailable: true }), {
  defaultTab: 'script',
  script: { command: `curl -fsSL https://raw.githubusercontent.com/nodera-studio/glen/main/install.sh | sh`, copy: true, note: 'Installs the latest release to ~/.local/bin. Add --version X.Y.Z or --prefix DIR after sh -s --.' },
  cargo,
  binary: { href: `${repo}/releases/latest`, label: 'Download the latest release', note: 'Archives for macOS (arm64, x86_64) and Linux (x86_64, arm64), with SHA-256 checksums.' },
 });
});

test('the script note names only flags install.sh documents', () => {
 const usage = readFileSync(new URL('../../../install.sh', import.meta.url), 'utf8');
 const { note } = installView({ ...site, releaseAvailable: true }).script;
 const flags = note.match(/--[a-z-]+/g);
 assert.deepEqual(flags, ['--version', '--prefix']);
 for (const flag of flags) assert.ok(usage.includes(`${flag} `), `${flag} is not in install.sh usage`);
 assert.ok(usage.includes('$HOME/.local/bin'), 'the default install dir the note promises');
});
