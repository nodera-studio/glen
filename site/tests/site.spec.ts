import { test, expect, type Page } from '@playwright/test';
import { compileTimeline, WINDOW_ROWS, LINE_STAGGER_MS, commandParts } from '../src/components/terminal/terminal-timeline.mjs';
import { scenes } from '../src/data/demo.mjs';
const cargo = 'cargo install --git https://github.com/nodera-studio/glen --locked';
const timeline=compileTimeline(scenes);
async function expectTerminalRows(page: Page, variant: string, rows: {scene: number; index: number}[]) {
 const window=await page.locator('.t-window').boundingBox();
 expect(window).not.toBeNull();
 for (const row of rows) {
  const locator=page.locator(`.t-${variant} [data-scene="${row.scene}"][data-row="${row.index}"]`);
  await expect(locator).toBeVisible();
  const styles=await locator.evaluate(el=>[el,...el.querySelectorAll('.t-piece,.t-cmd,.t-ps')].map(node=>{
   const style=getComputedStyle(node);
   return {visibility:style.visibility,opacity:style.opacity,clipPath:style.clipPath};
  }));
  for (const style of styles) {
   expect(style.visibility).toBe('visible');expect(style.opacity).toBe('1');
   expect(['none','inset(0px)','inset(0px 0px 0px 0px)','inset(0px 0% 0px 0px)']).toContain(style.clipPath);
  }
  const box=await locator.boundingBox();
  expect(box).not.toBeNull();
  expect(box!.x).toBeGreaterThanOrEqual(window!.x-.5);
  expect(box!.x+box!.width).toBeLessThanOrEqual(window!.x+window!.width+.5);
  expect(box!.y).toBeGreaterThanOrEqual(window!.y-.5);
  expect(box!.y+box!.height).toBeLessThanOrEqual(window!.y+window!.height+.5);
 }
}
function terminalRowsAt(time: number) {
 const offset=timeline.scroll.findLast(step=>step.time<=time)?.offset ?? 0;
 let position=0;
 return timeline.rows.filter(row=>{
  const top=row.units ? position : position-1;
  position+=row.units;
  return row.startMs<=time && (row.endMs===undefined || row.endMs>time) && top>=offset && top+(row.units || 1)<=offset+WINDOW_ROWS;
 });
}
async function hydrate(page: Page, section: string) {
 await page.locator(section).scrollIntoViewIfNeeded();
 await expect(page.locator(`${section} astro-island:not([ssr])`)).toHaveCount(1);
}
for (const [width,height] of [[360,800],[1280,800]]) for (const colorScheme of ['light','dark'] as const) {
 test(`${width}px ${colorScheme}: no overflow or runtime errors`, async ({page})=>{
  await page.setViewportSize({width:width!,height:height!});await page.emulateMedia({colorScheme});
  const errors:string[]=[];
  page.on('console',msg=>{if(msg.type()==='error') errors.push(msg.text());});
  page.on('pageerror',error=>errors.push(error.message));
  await page.goto('/');
  for (let y=0;y<await page.evaluate(()=>document.documentElement.scrollHeight);y+=500) {
   await page.evaluate(y=>window.scrollTo(0,y),y);await page.waitForTimeout(100);
  }
  await expect(page.locator('astro-island:not([ssr])')).toHaveCount(3);
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=document.documentElement.clientWidth)).toBe(true);
  expect(errors).toEqual([]);
  await expect(page.locator('body')).toHaveCSS('background-color',colorScheme==='light' ? 'rgb(20, 20, 20)' : 'rgb(237, 237, 237)');
  await expect(page.locator('main')).toHaveCSS('background-color',colorScheme==='light' ? 'rgb(255, 255, 255)' : 'rgb(25, 26, 28)');
  await page.screenshot({path:`test-results/site-${width}-${colorScheme}.png`,fullPage:true});
 });
}
test('theme choice persists; query override is view-only',async({page})=>{
 await page.goto('/');await page.locator('.theme-option:has(input[value=dark])').click();
 await expect(page.locator('#theme-dark')).toHaveAttribute('media','all');
 await expect(page.locator('html')).toHaveAttribute('data-theme','dark');
 await page.reload();await expect(page.locator('html')).toHaveAttribute('data-theme','dark');
 await page.goto('/?theme=light');await expect(page.locator('html')).toHaveAttribute('data-theme','light');
 await expect(page.locator('#theme-dark')).toHaveAttribute('media','not all');
 expect(await page.evaluate(()=>localStorage.getItem('glen-theme'))).toBe('dark');
 await page.locator('.theme-option:has(input[value=system])').click();
 await expect(page.locator('#theme-dark')).toHaveAttribute('media','(prefers-color-scheme: dark)');
});
test('storage exceptions fall back to system and still allow switching',async({page})=>{
 const errors:string[]=[];page.on('pageerror',e=>errors.push(e.message));
 await page.addInitScript(()=>{
  Storage.prototype.getItem=()=>{throw new Error('Storage denied');};
  Storage.prototype.setItem=()=>{throw new Error('Storage denied');};
 });
 await page.goto('/');await expect(page.locator('html')).toHaveAttribute('data-theme','system');
 await page.locator('.theme-option:has(input[value=dark])').click();
 await expect(page.locator('html')).toHaveAttribute('data-theme','dark');expect(errors).toEqual([]);
});
test.describe('without JavaScript',()=>{
 test.use({javaScriptEnabled:false,viewport:{width:360,height:800}});
 test('complete transcript and install alternatives remain available',async({page})=>{
  await page.goto('/');await expect(page.locator('.t-narrow [data-last-line]')).toBeVisible();
  await expect(page.locator('.t-transcript')).toContainText('glen ls');
  await expect(page.getByRole('heading',{level:1})).toBeVisible();
  await expect(page.locator('#install-panel-cargo code')).toHaveText(cargo);
  await expect(page.locator('#install-panel-cargo code')).toBeVisible();
  await expect(page.locator('#install-panel-script code')).toHaveText('curl -fsSL https://raw.githubusercontent.com/nodera-studio/glen/main/install.sh | sh');
  await expect(page.locator('#install-panel-script code')).toBeVisible();
  await expect(page.getByRole('link',{name:'Download from GitHub Releases'})).toBeVisible();
  await expect(page.locator('.theme-switch')).toBeHidden();await expect(page.locator('.t-controls')).toBeHidden();
  await expect(page.locator('#install-panel-cargo button')).toHaveCount(0);
  expect(await page.locator('.t-window').evaluate(e=>e.scrollHeight<=e.clientHeight+1)).toBe(true);
 });
});
test('reduced motion shows the full unclipped transcript immediately',async({page})=>{
 await page.setViewportSize({width:360,height:800});await page.emulateMedia({reducedMotion:'reduce'});await page.goto('/');
 const row=page.locator('.t-narrow [data-last-line]');await expect(row).toBeVisible();await expect(row).toHaveCSS('animation-name','none');
 expect(await page.locator('.t-window').evaluate(e=>e.scrollHeight<=e.clientHeight+1)).toBe(true);
 await expectTerminalRows(page,'narrow',timeline.rows.filter(row=>row.endMs===undefined));
 await hydrate(page,'#how');await expect(page.locator('#how svg[role="img"]')).toBeVisible();
});
for (const [width,variant] of [[360,'narrow'],[1280,'wide']] as const) test(`${width}px: terminal reveals each scene and a contiguous final tail`,async({page})=>{
 await page.setViewportSize({width,height:900});await page.emulateMedia({reducedMotion:'no-preference'});await page.goto('/');
 // Keep finished non-filling typing animations available for the later scene seeks.
 const animations=await page.locator('.terminal').evaluateHandle(el=>el.getAnimations({subtree:true}));
 const seek=(time: number)=>animations.evaluate(async(items,time)=>{
  items.forEach(animation=>animation.pause());
  await Promise.all(items.map(animation=>animation.ready));
  items.forEach(animation=>{ animation.currentTime=time; });
 },time);
 await page.waitForTimeout(timeline.totalMs+100);
 await expectTerminalRows(page,variant,terminalRowsAt(timeline.totalMs));
 for (const scene of [1,2]) await expect(page.locator(`.t-${variant} [data-scene="${scene}"][data-row="-1"]`)).toContainText(scenes[scene]!.cmd);
 const boxes=await page.locator(`.t-${variant} .t-row:not(.t-transient)`).evaluateAll(rows=>rows.map(row=>{const box=row.getBoundingClientRect();return {y:box.y,bottom:box.bottom};}).filter(box=>box.bottom>document.querySelector('.t-window')!.getBoundingClientRect().top+.5));
 for (let i=1;i<boxes.length;i++) expect(Math.abs(boxes[i]!.y-boxes[i-1]!.bottom)).toBeLessThan(.5);
 for (const scene of scenes.keys()) {
  const end=Math.max(...timeline.rows.filter(row=>row.scene===scene).map(row=>Math.max(row.startMs+(row.type?.durationMs ?? 0),row.endMs ?? 0)))+LINE_STAGGER_MS;
  await seek(end);
  await expectTerminalRows(page,variant,terminalRowsAt(end));
 }
 for (const row of timeline.rows.filter(row=>row.type)) {
  const command=scenes[row.scene]!.cmd;
  for (const [index,part] of commandParts(command).entries()) {
   const selector=`.t-${variant} [data-scene="${row.scene}"][data-row="-1"] .t-cmd`;
   const end=await animations.evaluate((items,{selector,index})=>{
    const target=document.querySelectorAll(selector)[index];
    const animation=items.find(animation=>(animation as CSSAnimation).animationName==='t-type' && (animation.effect as KeyframeEffect).target===target);
    if (!animation) throw new Error(`Missing typing animation: ${selector} part ${index}`);
    return Number(animation.effect!.getComputedTiming().endTime);
   },{selector,index});
   expect(end).toBeCloseTo(row.startMs+row.type!.durationMs*(part.offset+part.text.length)/command.length,6);
   await seek(end-1);
   const clip=await page.locator(selector).nth(index).evaluate(el=>getComputedStyle(el).clipPath);
   expect(Number(clip.match(/^inset\(0px ([\d.]+)% 0px 0px\)$/)?.[1])).toBeCloseTo(100/part.text.length,4);
   for (const time of [end,end+1]) {
    await seek(time);
    await expect(page.locator(selector).nth(index)).toHaveCSS('clip-path','none');
   }
  }
 }
 await animations.dispose();
});
test('terminal controls have at least 24px hit areas',async({page})=>{
 await page.goto('/');
 for (const selector of ['.t-pause-label','.t-replay']) {
  const box=await page.locator(selector).boundingBox();expect(box!.width).toBeGreaterThanOrEqual(24);expect(box!.height).toBeGreaterThanOrEqual(24);
 }
});
test('CSS pause freezes every row; replay restarts',async({page})=>{
 await page.setViewportSize({width:360,height:800});await page.goto('/');await page.check('#t-pause');
 expect(await page.locator('.t-row').evaluateAll(rows=>rows.every(row=>getComputedStyle(row).animationPlayState.split(',').every(s=>s.trim()==='paused')))).toBe(true);
 await page.getByRole('button',{name:'Replay',exact:true}).click();await expect(page.locator('#t-pause')).not.toBeChecked();
 await expect(page.locator('.t-narrow [data-last-line]')).toHaveCSS('visibility','hidden');
});
for (const section of ['#install','#agents']) test(`${section} tabs support automatic keyboard activation`,async({page})=>{
 await page.goto('/');await hydrate(page,section);
 const tabs=page.locator(`${section} [role=tab]`);const selected=page.locator(`${section} [role=tab][aria-selected=true]`);
 const index=await selected.evaluate(el=>Array.from(el.parentElement!.children).indexOf(el));
 await selected.focus();await page.keyboard.press('ArrowRight');
 const next=tabs.nth((index+1)%await tabs.count());await expect(next).toHaveAttribute('aria-selected','true');await expect(next).toBeFocused();
 await expect(page.locator(`#${await next.getAttribute('aria-controls')}`)).toBeVisible();
 await expect(page.locator(`${section} [role=tabpanel]:not([hidden])`)).toHaveCount(1);
 await expect(page.locator(`${section} [role=tab][aria-selected=true]`)).toHaveCount(1);
 await page.keyboard.press('End');await expect(tabs.last()).toBeFocused();await expect(tabs.last()).toHaveAttribute('aria-selected','true');
 await page.keyboard.press('Home');await expect(tabs.first()).toBeFocused();await expect(tabs.first()).toHaveAttribute('aria-selected','true');
 await page.keyboard.press('ArrowLeft');await expect(tabs.last()).toBeFocused();await expect(tabs.last()).toHaveAttribute('aria-selected','true');
});
test('copy writes the exact command and announces success',async({page,context})=>{
 await context.grantPermissions(['clipboard-read','clipboard-write']);await page.goto('/');await hydrate(page,'#install');
 await page.getByRole('button',{name:'Copy Cargo command',exact:true}).click();
 expect(await page.evaluate(()=>navigator.clipboard.readText())).toBe(cargo);
 await expect(page.locator('#install-panel-cargo [role=status]')).toHaveText('Copied');
 await page.locator('#install-panel-cargo [role=status]').evaluate(el=>{
  const changes:string[]=[];
  new MutationObserver(()=>changes.push(el.textContent ?? '')).observe(el,{childList:true,characterData:true,subtree:true});
  Object.assign(window,{copyStatusChanges:changes});
 });
 await page.getByRole('button',{name:'Copied Cargo command',exact:true}).click();
 await expect(page.locator('#install-panel-cargo [role=status]')).toHaveText('Copied');
 await expect.poll(()=>page.evaluate(()=>Reflect.get(window,'copyStatusChanges'))).toEqual(['','Copied']);
 await expect(page.getByRole('button',{name:'Copied Cargo command',exact:true})).toContainText('Copied');
});
for (const failure of ['missing','rejected']) test(`copy announces ${failure} Clipboard API failure`,async({page})=>{
 await page.addInitScript(failure=>Object.defineProperty(navigator,'clipboard',{configurable:true,value:failure==='missing' ? undefined : {writeText:()=>Promise.reject(new Error('Clipboard denied'))}}),failure);
 await page.goto('/');await hydrate(page,'#install');await page.getByRole('button',{name:'Copy Cargo command',exact:true}).click();
 await expect(page.locator('#install-panel-cargo [role=status]')).toHaveText("Couldn't copy. Select the command and copy it yourself.");
 await expect(page.getByRole('button',{name:'Copy failed Cargo command',exact:true})).toContainText('Copy failed');
});
test('412px first viewport requests no JavaScript file',async({page})=>{
 await page.setViewportSize({width:412,height:823});const scripts:string[]=[];
 page.on('request',request=>{if(request.resourceType()==='script') scripts.push(request.url());});
 await page.goto('/');await page.waitForTimeout(2000);expect(scripts).toEqual([]);
 await expect(page.getByRole('heading',{level:1})).toBeVisible();await expect(page.locator('.t-narrow .t-row').first()).toBeVisible();
});
test('Cargo defaults; unreleased downloads carry release labels',async({page})=>{
 await page.goto('/');await hydrate(page,'#install');
 await expect(page.getByRole('tab',{name:'Cargo',exact:true})).toHaveAttribute('aria-selected','true');
 for (const name of ['Script','Binary']) {
  await page.getByRole('tab',{name,exact:true}).click();
  await expect(page.locator('#install [role=tabpanel]:not([hidden])')).toContainText('Available with the v0.1.0 release.');
 }
 await page.getByRole('tab',{name:'Script',exact:true}).click();
 await expect(page.locator('#install-panel-script button')).toHaveCount(0);
});
