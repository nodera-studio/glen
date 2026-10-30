import { useState } from 'react';
import type { Site } from '../../data/site';
import { IslandRoot } from './IslandRoot';
import { ExpandableTabs } from '../morphin/navigation-tabs/ExpandableTabs';
import { CopyButton } from '../morphin/border-button/CopyButton';
const icon = (d: string) => <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d={d} /></svg>;
const tabs = [
 {id:'script',title:'Script',icon:icon('m3 4.5 3.5 3.5L3 11.5m5 0h5')},
 {id:'cargo',title:'Cargo',icon:icon('m8 1.75 5.5 3.1v6.3L8 14.25l-5.5-3.1v-6.3L8 1.75Zm0 6.2v6.3m-5.5-9.4L8 7.95l5.5-3.1')},
 {id:'binary',title:'Binary',icon:icon('M8 2.5v7.5m-3-3 3 3 3-3m-8.5 3.5v2.5h11V10.5')},
];
const notes: Record<string,string> = { script: 'Installs the latest release binary.', cargo: 'Builds from source with a Rust toolchain.', binary: 'Pick the archive for your platform.' };
export default function InstallTabs({commands,releasesUrl,releaseAvailable}:Pick<Site,'commands'|'releasesUrl'|'releaseAvailable'>) {
 const [selected,setSelected] = useState(releaseAvailable ? 'script' : 'cargo');
 return <IslandRoot><div data-tabs>
  <ExpandableTabs labels="always" tabs={tabs} selectedId={selected} onSelect={setSelected} label="Install method" idPrefix="install" />
  {tabs.map(tab=><div key={tab.id} role="tabpanel" id={`install-panel-${tab.id}`} aria-labelledby={`install-tab-${tab.id}`} tabIndex={0} hidden={selected!==tab.id} className="focus-ring">
   <h3 className="tab-heading sr-only">{tab.title}</h3>
   {tab.id==='binary' ? <a className="focus-ring quiet-link" href={releasesUrl}>Download from GitHub Releases</a> : <div className="command-row"><pre className="command"><code className="bare">{tab.id==='script' ? commands.script : commands.cargo}</code></pre>{(tab.id!=='script' || releaseAvailable) && <CopyButton text={tab.id==='script' ? commands.script : commands.cargo} label={`${tab.title} command`} />}</div>}
   <p className="panel-note">{notes[tab.id]}{!releaseAvailable && tab.id!=='cargo' && ' Available with the v0.1.0 release.'}</p>
  </div>)}
 </div></IslandRoot>;
}
