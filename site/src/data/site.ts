const repoUrl = 'https://github.com/nodera-studio/glen';
const installScriptUrl = 'https://raw.githubusercontent.com/nodera-studio/glen/main/install.sh';
export const site = {
 repoUrl, releasesUrl: repoUrl + '/releases', latestReleaseUrl: repoUrl + '/releases/latest', installScriptUrl, testedOmp: '18.14.0',
 // v0.1.0 and its 9 assets are published, so the page shows the install script and the latest release; check-links.mjs --online verifies both before any deploy.
 version: '0.1.0', releaseAvailable: true,
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
