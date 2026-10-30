import { useState } from 'react';
import type { Site } from '../../data/site';
import { IslandRoot } from './IslandRoot';
import { ExpandableTabs } from '../morphin/navigation-tabs/ExpandableTabs';
import { CopyButton } from '../morphin/border-button/CopyButton';
const tabs = [
 {id:'script',title:'Script',icon:<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="m5 7 5 5-5 5m8 0h6"/></svg>},
 {id:'cargo',title:'Cargo',icon:<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="m12 3 9 5v9l-9 5-9-5V8l9-5Zm0 9v10M3 8l9 4 9-4M7 6l9 4"/></svg>},
 {id:'binary',title:'Binary',icon:<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5"/></svg>},
];
export default function InstallTabs({commands,releasesUrl,releaseAvailable}:Pick<Site,'commands'|'releasesUrl'|'releaseAvailable'>) {
 const [selected,setSelected] = useState(releaseAvailable ? 'script' : 'cargo');
 return <IslandRoot><div data-tabs>
  <ExpandableTabs labels="always" tabs={tabs} selectedId={selected} onSelect={setSelected} label="Install method" idPrefix="install" />
  {tabs.map(tab=><div key={tab.id} role="tabpanel" id={`install-panel-${tab.id}`} aria-labelledby={`install-tab-${tab.id}`} tabIndex={0} hidden={selected!==tab.id} className="focus-ring rounded-lg border border-line bg-layer-1 p-5">
   <h3 className="tab-heading sr-only">{tab.title}</h3>
   {tab.id==='binary' ? <a className="focus-ring text-ink underline" href={releasesUrl}>Download from GitHub Releases</a> : <div className="command-row"><pre className="command min-w-0 flex-1"><code>{tab.id==='script' ? commands.script : commands.cargo}</code></pre>{(tab.id!=='script' || releaseAvailable) && <CopyButton text={tab.id==='script' ? commands.script : commands.cargo} label={`${tab.title} command`} />}</div>}
   <p className="mt-3 text-body-sm text-ink-subdued">{tab.id==='script' ? 'Installs the latest release binary.' : tab.id==='cargo' ? 'Builds from source with a Rust toolchain.' : 'Pick the archive for your platform.'}</p>
   {!releaseAvailable && tab.id!=='cargo' && <p className="mt-2 text-body-sm text-ink-subdued">Available with the v0.1.0 release.</p>}
  </div>)}
 </div></IslandRoot>;
}
