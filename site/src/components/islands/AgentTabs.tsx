import { useState } from 'react';
import { site } from '../../data/site';
import { IslandRoot } from './IslandRoot';
import { ExpandableTabs } from '../morphin/navigation-tabs/ExpandableTabs';
import { CopyButton } from '../morphin/border-button/CopyButton';
const tabs = [
 {id:'claude',title:'Claude Code',icon:<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="M12 2v20M2 12h20M5 5l14 14M5 19 19 5"/></svg>},
 {id:'codex',title:'Codex',icon:<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="m8 5-6 7 6 7m8-14 6 7-6 7M14 3l-4 18"/></svg>},
];
export default function AgentTabs() {
 const [selected,setSelected] = useState('claude');
 return <IslandRoot><div data-tabs>
  <ExpandableTabs labels="always" tabs={tabs} selectedId={selected} onSelect={setSelected} label="Coding agent" idPrefix="agent" />
  {tabs.map(tab=>{ const command=tab.id==='claude' ? site.commands.skillClaude : site.commands.skillCodex;
   return <div key={tab.id} role="tabpanel" id={`agent-panel-${tab.id}`} aria-labelledby={`agent-tab-${tab.id}`} tabIndex={0} hidden={selected!==tab.id} className="focus-ring rounded-lg border border-line bg-layer-1 p-5">
    <h3 className="tab-heading sr-only">{tab.title}</h3>
    <div className="command-row"><pre className="command min-w-0 flex-1"><code>{command}</code></pre><CopyButton text={command} label={`${tab.title} skill command`} /></div>
    <p className="mt-3 text-body-sm text-ink-subdued">Writes <code className="break-all">{tab.id==='claude' ? site.skillPaths.claude : site.skillPaths.codex}</code>.{tab.id==='claude' && ' Claude Code picks it up in the next session.'}</p>
    {tab.id==='codex' && <p className="mt-3 text-body-sm text-ink-subdued">Give glen commands a timeout of at least 100 seconds, or use <code>glen send --no-wait</code> and then <code>glen wait &lt;job&gt;</code>.</p>}
   </div>;
  })}
  <p className="mt-4 text-body-sm text-ink-subdued">Add <code>--project</code> to install into this repository instead.</p>
 </div></IslandRoot>;
}
