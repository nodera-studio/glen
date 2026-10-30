import { useState } from 'react';
import { site } from '../../data/site';
import { IslandRoot } from './IslandRoot';
import { ExpandableTabs } from '../morphin/navigation-tabs/ExpandableTabs';
import { CopyButton } from '../morphin/border-button/CopyButton';
const icon = (d: string) => <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d={d} /></svg>;
const tabs = [
 {id:'claude',title:'Claude Code',icon:icon('M8 2v12M2 8h12M3.8 3.8l8.4 8.4m0-8.4-8.4 8.4')},
 {id:'codex',title:'Codex',icon:icon('M5.5 4 2 8l3.5 4m5-8L14 8l-3.5 4')},
];
export default function AgentTabs() {
 const [selected,setSelected] = useState('claude');
 return <IslandRoot><div data-tabs>
  <ExpandableTabs labels="always" tabs={tabs} selectedId={selected} onSelect={setSelected} label="Coding agent" idPrefix="agent" />
  {tabs.map(tab=>{ const command=tab.id==='claude' ? site.commands.skillClaude : site.commands.skillCodex;
   return <div key={tab.id} role="tabpanel" id={`agent-panel-${tab.id}`} aria-labelledby={`agent-tab-${tab.id}`} tabIndex={0} hidden={selected!==tab.id} className="focus-ring">
    <h3 className="tab-heading sr-only">{tab.title}</h3>
    <div className="command-row"><pre className="command"><code className="bare">{command}</code></pre><CopyButton text={command} label={`${tab.title} skill command`} /></div>
    <p className="panel-note">Writes <code>{tab.id==='claude' ? site.skillPaths.claude : site.skillPaths.codex}</code>.{tab.id==='claude' ? ' Claude Code picks it up in its next session.' : ' Give glen commands a timeout of at least 100 seconds, or use '}{tab.id==='codex' && <><code>glen send --wait 5s</code> and repeat <code>glen wait &lt;job&gt; --timeout 5s</code>.</>}</p>
   </div>;
  })}
  <p className="tabs-foot">Add <code>--project</code> to install into this repository instead.</p>
 </div></IslandRoot>;
}
