// The supported omp range has one source, src/omp/compat.json, which Rust and the nightly read too.
import compat from '../../../src/omp/compat.json' with { type: 'json' };
const repoUrl = 'https://github.com/nodera-studio/glen';
const installScriptUrl = 'https://raw.githubusercontent.com/nodera-studio/glen/main/install.sh';
export const site = {
 repoUrl, releasesUrl: repoUrl + '/releases', latestReleaseUrl: repoUrl + '/releases/latest', installScriptUrl,
 testedOmp: compat.tested,
 // "18.14.0" while min and max_tested agree, "18.14.0 to 18.16.0" once the range widens.
 supportedOmp: compat.min === compat.max_tested ? compat.min : `${compat.min} to ${compat.max_tested}`,
 // The current release, kept equal to Cargo.toml's version (site.test.mjs): the release PR bumps both. Every release
 // since v0.1.0 publishes 9 assets, so the page shows the install script and the latest release; check-links.mjs --online verifies both before any deploy.
 version: '0.1.1', releaseAvailable: true,
 commands: {
  script: `curl -fsSL ${installScriptUrl} | sh`,
  cargo: `cargo install --git ${repoUrl} --locked`, doctor: 'glen doctor',
  extension: 'glen install-extension', skillClaude: 'glen install-skill --claude', skillCodex: 'glen install-skill --codex',
  mcpClaude: 'claude mcp add glen -- glen mcp', mcpCodex: 'codex mcp add glen -- glen mcp',
 },
 skillPaths: { claude: '~/.claude/skills/glen/SKILL.md', codex: '~/.agents/skills/glen/SKILL.md' },
 mcpGuideUrl: repoUrl + '/blob/main/README.md#use-from-an-mcp-host-glen-mcp',
};
export type Site = typeof site;
/** The header, hero and footer version chip. A bare version only once it is released: check-links.mjs fails a pre-release build whose chip lacks "coming soon". */
export const versionLabel = ({ version, releaseAvailable }: Pick<Site, 'version' | 'releaseAvailable'>) => releaseAvailable ? `v${version}` : `v${version} · coming soon`;
