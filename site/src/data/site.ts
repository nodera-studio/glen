const repoUrl = 'https://github.com/nodera-studio/glen';
const installScriptUrl = 'https://raw.githubusercontent.com/nodera-studio/glen/main/install.sh';
export const site = {
 repoUrl, releasesUrl: repoUrl + '/releases', installScriptUrl, testedOmp: '18.14.0',
 // W7 flips this after install.sh and the v0.1.0 release assets are published.
 releaseAvailable: false,
 commands: {
  script: `curl -fsSL ${installScriptUrl} | sh`,
  cargo: `cargo install --git ${repoUrl} --locked`, doctor: 'glen doctor',
  extension: 'glen install-extension', skillClaude: 'glen install-skill --claude', skillCodex: 'glen install-skill --codex',
 },
 skillPaths: { claude: '~/.claude/skills/glen/SKILL.md', codex: '~/.agents/skills/glen/SKILL.md' },
};
export type Site = typeof site;
