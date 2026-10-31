const repoUrl = 'https://github.com/nodera-studio/glen';
const installScriptUrl = 'https://raw.githubusercontent.com/nodera-studio/glen/main/install.sh';
export const site = {
 repoUrl, releasesUrl: repoUrl + '/releases', latestReleaseUrl: repoUrl + '/releases/latest', installScriptUrl, testedOmp: '18.14.0',
 // W8 flips releaseAvailable after v0.1.0 and its assets are published; check-links.mjs --online then verifies the script and latest-release URLs before any deploy.
 version: '0.1.0', releaseAvailable: false,
 commands: {
  script: `curl -fsSL ${installScriptUrl} | sh`,
  cargo: `cargo install --git ${repoUrl} --locked`, doctor: 'glen doctor',
  extension: 'glen install-extension', skillClaude: 'glen install-skill --claude', skillCodex: 'glen install-skill --codex',
 },
 skillPaths: { claude: '~/.claude/skills/glen/SKILL.md', codex: '~/.agents/skills/glen/SKILL.md' },
};
export type Site = typeof site;
/** The header, hero and footer version chip. A bare version only once it is released: check-links.mjs fails a pre-release build whose chip lacks "coming soon". */
export const versionLabel = ({ version, releaseAvailable }: Pick<Site, 'version' | 'releaseAvailable'>) => releaseAvailable ? `v${version}` : `v${version} · coming soon`;
