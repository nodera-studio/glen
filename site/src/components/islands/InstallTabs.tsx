import { useState } from 'react';
import type { installView } from '../../data/install.mjs';
import { IslandRoot } from './IslandRoot';
import { ExpandableTabs } from '../morphin/navigation-tabs/ExpandableTabs';
import { CopyButton } from '../morphin/border-button/CopyButton';
const icon = (d: string) => <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d={d} /></svg>;
const tabs = [
 {id:'script',title:'Script',icon:icon('m3 4.5 3.5 3.5L3 11.5m5 0h5')},
 {id:'cargo',title:'Cargo',icon:icon('m8 1.75 5.5 3.1v6.3L8 14.25l-5.5-3.1v-6.3L8 1.75Zm0 6.2v6.3m-5.5-9.4L8 7.95l5.5-3.1')},
 {id:'binary',title:'Binary',icon:icon('M8 2.5v7.5m-3-3 3 3 3-3m-8.5 3.5v2.5h11V10.5')},
];
type View = ReturnType<typeof installView>;
const command = (text: string, label: string, copy: boolean) => <div className="command-row"><pre className="command"><code className="bare">{text}</code></pre>{copy && <CopyButton text={text} label={label} />}</div>;
// Keeps each --flag whole: at phone widths the note otherwise breaks "--version" after its dashes.
const unbrokenFlags = (text: string) => text.split(/(--[a-z][a-z-]*)/).map((part, i) => i % 2 ? <span key={i} className="whitespace-nowrap">{part}</span> : part);
// The page carries only what installView() exposes: until the first release, no installer URL.
export default function InstallTabs({view}:{view:View}) {
 const [selected,setSelected] = useState<string>(view.defaultTab);
 const panel = (id: string, title: string) => {
  if (id==='binary') return <><a className="focus-ring quiet-link" href={view.binary.href}>{view.binary.label}</a><p className="panel-note">{view.binary.note}</p></>;
  if (id==='cargo') return <>{command(view.cargo.command, `${title} command`, true)}<p className="panel-note">{view.cargo.note}</p></>;
  if (view.script.command===null) return <p className="panel-note pending-note">{view.script.note}</p>;
  return <>{command(view.script.command, `${title} command`, view.script.copy)}<p className="panel-note">{unbrokenFlags(view.script.note)}</p></>;
 };
 return <IslandRoot><div data-tabs>
  <ExpandableTabs labels="always" tabs={tabs} selectedId={selected} onSelect={setSelected} label="Install method" idPrefix="install" />
  {tabs.map(tab=><div key={tab.id} role="tabpanel" id={`install-panel-${tab.id}`} aria-labelledby={`install-tab-${tab.id}`} tabIndex={0} hidden={selected!==tab.id} className="focus-ring">
   <h3 className="tab-heading sr-only">{tab.title}</h3>
   {panel(tab.id, tab.title)}
  </div>)}
 </div></IslandRoot>;
}
