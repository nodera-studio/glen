import { test, expect, type Page } from '@playwright/test';
const cargo = 'cargo install --git https://github.com/nodera-studio/glen --locked';
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
 await hydrate(page,'#how');await expect(page.locator('#how svg[role="img"]')).toBeVisible();
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
});
for (const failure of ['missing','rejected']) test(`copy announces ${failure} Clipboard API failure`,async({page})=>{
 await page.addInitScript(failure=>Object.defineProperty(navigator,'clipboard',{configurable:true,value:failure==='missing' ? undefined : {writeText:()=>Promise.reject(new Error('Clipboard denied'))}}),failure);
 await page.goto('/');await hydrate(page,'#install');await page.getByRole('button',{name:'Copy Cargo command',exact:true}).click();
 await expect(page.locator('#install-panel-cargo [role=status]')).toHaveText("Couldn't copy. Select the command and copy it yourself.");
 await expect(page.getByRole('button',{name:'Copy Cargo command',exact:true})).toContainText('Copy failed');
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
});
