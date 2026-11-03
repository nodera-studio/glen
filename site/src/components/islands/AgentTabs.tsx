import { useState } from 'react';
import type { Site } from '../../data/site';
import { IslandRoot } from './IslandRoot';
import { ExpandableTabs } from '../morphin/navigation-tabs/ExpandableTabs';
import { CopyButton } from '../morphin/border-button/CopyButton';
const icon = (d: string) => <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d={d} /></svg>;
const tabs = [
 {id:'claude',title:'Claude Code',icon:icon('M8 2v12M2 8h12M3.8 3.8l8.4 8.4m0-8.4-8.4 8.4')},
 {id:'codex',title:'Codex',icon:icon('M5.5 4 2 8l3.5 4m5-8L14 8l-3.5 4')},
 {id:'mcp',title:'MCP',icon:icon('M6 1.75v3m4-3v3M4 4.75h8v2.5a4 4 0 0 1-8 0v-2.5Zm4 6.5v3')},
];
type Commands = Pick<Site['commands'],'skillClaude'|'skillCodex'|'mcpClaude'|'mcpCodex'>;
const command = (text: string, label: string) => <div className="command-row"><pre className="command"><code className="bare">{text}</code></pre><CopyButton text={text} label={label} /></div>;
const job = <code>glen wait &lt;job&gt; --for reply</code>;
// Props, not an import of the site data: an island bundles what it imports, and that module
// holds the installer URL, which must stay off the page until the first release.
export default function AgentTabs({commands,skillPaths,mcpGuideUrl}:{commands:Commands,skillPaths:Site['skillPaths'],mcpGuideUrl:Site['mcpGuideUrl']}) {
 const [selected,setSelected] = useState('claude');
 const panel = (id: string, title: string) => {
  if (id==='claude') return <>{command(commands.skillClaude, `${title} skill command`)}<p className="panel-note">Writes <code>{skillPaths.claude}</code>. Claude Code picks it up in its next session. Replies need no polling: it runs {job} in the background and wakes when it returns.</p></>;
  if (id==='codex') return <>{command(commands.skillCodex, `${title} skill command`)}<p className="panel-note">Writes <code>{skillPaths.codex}</code>. Codex has no push: it blocks on {job} instead. Give glen commands a timeout of at least 100 seconds, or use <code>glen send --wait 5s</code> and repeat <code>glen wait &lt;job&gt; --timeout 5s</code>.</p></>;
  return <>{command(commands.mcpClaude, 'Claude Code MCP command')}{command(commands.mcpCodex, 'Codex MCP command')}
   <p className="panel-note"><code>glen mcp</code> serves 14 tools, such as <code>glen_send</code> and <code>glen_wait</code>, each returning the command's <code>--json</code> envelope. Codex stops a tool after 60 seconds by default, so set <code>tool_timeout_sec = 120</code>. Keep per-tool approval on: a call can send into omp sessions.</p>
   <p className="panel-note"><a className="focus-ring quiet-link" href={mcpGuideUrl}>MCP setup in the README <svg aria-hidden="true" viewBox="0 0 16 16"><path d="M3 8h10m-4-4 4 4-4 4" /></svg></a></p></>;
 };
 return <IslandRoot><div data-tabs>
  <ExpandableTabs labels="always" tabs={tabs} selectedId={selected} onSelect={setSelected} label="Agent setup" idPrefix="agent" />
  {tabs.map(tab=><div key={tab.id} role="tabpanel" id={`agent-panel-${tab.id}`} aria-labelledby={`agent-tab-${tab.id}`} tabIndex={0} hidden={selected!==tab.id} className="focus-ring">
   <h3 className="tab-heading sr-only">{tab.title}</h3>
   {panel(tab.id, tab.title)}
  </div>)}
  <p className="tabs-foot" hidden={selected==='mcp'}>Add <code>--project</code> to install the skill into the current directory instead (run it from the repository root).</p>
 </div></IslandRoot>;
}
