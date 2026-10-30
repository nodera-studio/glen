import { test, expect, type Page } from '@playwright/test';
const cargo = 'cargo install --git https://github.com/nodera-studio/glen --locked';

// Rows a reader can see in the variant that is showing, pane by pane: rendered rows that sit inside
// the pane's viewport once the scroll transition settles, so a wrong scroll offset changes the text.
async function paneText(page: Page) {
 return page.locator('#term').evaluate(async fig => {
  await Promise.all(fig.getAnimations({ subtree: true }).filter(a => a instanceof CSSTransition).map(a => a.finished.catch(() => {})));
  const variant = [...fig.querySelectorAll<HTMLElement>('.term-v')].find(v => v.offsetParent)!;
  return [...variant.querySelectorAll<HTMLElement>('.term-pane')].filter(p => getComputedStyle(p).display !== 'none').map(pane => {
   const vp = pane.querySelector('.term-vp')!.getBoundingClientRect();
   return [...pane.querySelectorAll<HTMLElement>('.tl[data-at]')].filter(r => {
    const style = getComputedStyle(r), box = r.getBoundingClientRect();
    return style.display !== 'none' && style.visibility !== 'hidden' && box.top >= vp.top - 1 && box.bottom <= vp.bottom + 1;
   }).map(r => r.textContent ?? '');
  });
 });
}
const termRows = (page: Page) => page.locator('#term').evaluate(fig => +getComputedStyle([...fig.querySelectorAll<HTMLElement>('.term-v')].find(v => v.offsetParent)!).getPropertyValue('--rows'));
const doneAt = async (page: Page) => (JSON.parse((await page.locator('#term').getAttribute('data-cues'))!) as [number, string][]).find(c => c[1] === 'done')![0];
const visibleRow = (page: Page, variant: 'w' | 'n', pane: 'a' | 'b', text: string | RegExp) => page.locator(`.term-v[data-v=${variant}] .term-pane[data-p=${pane}] .tl.on`, { hasText: text });
async function hydrate(page: Page, section: string) {
 await page.locator(section).scrollIntoViewIfNeeded();
 await expect(page.locator(`${section} astro-island:not([ssr])`).first()).toBeAttached();
}
async function scrollThrough(page: Page) {
 for (let y = 0; y < await page.evaluate(() => document.documentElement.scrollHeight); y += 500) {
  await page.evaluate(y => window.scrollTo(0, y), y); await page.waitForTimeout(100);
 }
}

for (const [width, height] of [[360, 800], [390, 844], [1280, 800]]) for (const colorScheme of ['light', 'dark'] as const) {
 test(`${width}px ${colorScheme}: no overflow or runtime errors`, async ({ page }) => {
  await page.setViewportSize({ width: width!, height: height! }); await page.emulateMedia({ colorScheme });
  const errors: string[] = [];
  page.on('console', msg => { if (msg.type() === 'error') errors.push(msg.text()); });
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('/');
  await scrollThrough(page);
  await expect(page.locator('astro-island:not([ssr])')).toHaveCount(4);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
  expect(errors).toEqual([]);
  await expect(page.locator('body')).toHaveCSS('background-color', colorScheme === 'light' ? 'rgb(20, 20, 20)' : 'rgb(237, 237, 237)');
  await expect(page.locator('main')).toHaveCSS('background-color', colorScheme === 'light' ? 'rgb(255, 255, 255)' : 'rgb(25, 26, 28)');
 });
}

test('theme choice persists; query override is view-only', async ({ page }) => {
 await page.goto('/'); await page.locator('.theme-option:has(input[value=dark])').click();
 await expect(page.locator('#theme-dark')).toHaveAttribute('media', 'all');
 await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
 await page.reload(); await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
 await page.goto('/?theme=light'); await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
 await expect(page.locator('#theme-dark')).toHaveAttribute('media', 'not all');
 expect(await page.evaluate(() => localStorage.getItem('glen-theme'))).toBe('dark');
 await page.locator('.theme-option:has(input[value=system])').click();
 await expect(page.locator('#theme-dark')).toHaveAttribute('media', '(prefers-color-scheme: dark)');
});
test('storage exceptions fall back to system and still allow switching', async ({ page }) => {
 const errors: string[] = []; page.on('pageerror', e => errors.push(e.message));
 await page.addInitScript(() => {
  Storage.prototype.getItem = () => { throw new Error('Storage denied'); };
  Storage.prototype.setItem = () => { throw new Error('Storage denied'); };
 });
 await page.goto('/'); await expect(page.locator('html')).toHaveAttribute('data-theme', 'system');
 await page.locator('.theme-option:has(input[value=dark])').click();
 await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark'); expect(errors).toEqual([]);
});

test.describe('without JavaScript', () => {
 test.use({ javaScriptEnabled: false, viewport: { width: 360, height: 800 } });
 test('complete transcript, install alternatives and FAQ remain available', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'static');
  await expect(page.locator('.term-v[data-v=n] .tl[data-k=prompt]').last()).toBeVisible();
  await expect(page.locator('.term-v[data-v=n] .tl[data-k=spin]')).toBeHidden();
  await expect(page.locator('#term .sr-only')).toContainText('glen ls');
  await expect(page.locator('#term .sr-only')).toContainText('↩ Added the index. All 12 tests pass. ✓');
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
  await expect(page.locator('#install-panel-cargo code')).toHaveText(cargo);
  await expect(page.locator('#install-panel-cargo code')).toBeVisible();
  // Until the first release the Script panel is a note: no installer command to copy (W7 D9).
  await expect(page.locator('#install-panel-script')).toContainText('The install script ships with the first release (v0.1.0). Use Cargo until then.');
  await expect(page.locator('#install-panel-script code')).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'GitHub Releases', exact: true })).toBeVisible();
  await expect(page.getByRole('link', { name: 'GitHub Releases', exact: true })).toHaveAttribute('href', 'https://github.com/nodera-studio/glen/releases');
  await expect(page.locator('.theme-switch')).toBeHidden(); await expect(page.locator('.term-ctl')).toBeHidden();
  await expect(page.locator('#install-panel-cargo button')).toHaveCount(0);
  for (const pane of await page.locator('.term-v[data-v=n] .term-vp').all()) expect(await pane.evaluate(e => e.scrollHeight <= e.clientHeight + 1)).toBe(true);
  // How it works shows each finished stage.
  await expect(page.locator('[data-stage=hear] [data-before]')).toBeHidden();
  await expect(page.locator('[data-stage=hear] [data-after]').last()).toBeVisible();
  const faq = page.locator('#faq details').first();
  await faq.locator('summary').click(); await expect(faq).toHaveAttribute('open', '');
 });
 for (const width of [360, 1280]) test(`${width}px: the ?term=end frame is the tail of the static transcript`, async ({ page, browser }) => {
  await page.setViewportSize({ width, height: 900 });
  await page.goto('/');
  // Without JS the panes grow to fit, so every row is visible.
  const withoutJs = await paneText(page);
  const js = await browser.newPage({ viewport: { width, height: 900 }, javaScriptEnabled: true });
  await js.goto('/?term=end');
  await expect(js.locator('#term')).toHaveAttribute('data-term', 'done');
  // With JS the narrow window shows one pane at a time; compare pane by pane.
  let shown: string[][] = [];
  if (width < 760) for (const p of ['a', 'b']) { await js.locator(`.term-tab[data-p=${p}]`).click(); shown.push(...await paneText(js)); }
  else shown = await paneText(js);
  const rows = await termRows(js);
  expect(shown).toHaveLength(withoutJs.length);
  withoutJs.forEach((all, i) => {
   // The end frame shows the newest rows, the transcript's tail, and fills the pane but for the cursor line.
   expect(shown[i]!.length).toBeGreaterThanOrEqual(Math.min(all.length, rows - 1));
   expect(shown[i]).toEqual(all.slice(-shown[i]!.length));
  });
  // The omp pane outgrows the window, so this frame really scrolled.
  expect(shown[1]!.length).toBeLessThan(withoutJs[1]!.length);
  await js.close();
 });
});

test('reduced motion shows the final frame, static', async ({ page }) => {
 await page.setViewportSize({ width: 390, height: 844 }); await page.emulateMedia({ reducedMotion: 'reduce' }); await page.goto('/');
 await expect(page.locator('#term')).toHaveAttribute('data-term', 'static');
 await expect(page.locator('.term-ctl button').first()).toBeHidden();
 const rows = page.locator('.term-v[data-v=n] .tl[data-at]:not([data-k=spin])');
 for (const row of await rows.all()) await expect(row).toBeVisible();
 await expect(page.locator('.term-v[data-v=n] .tl[data-k=prompt] .cur')).toHaveCSS('animation-name', 'none');
 for (const pane of await page.locator('.term-v[data-v=n] .term-vp').all()) expect(await pane.evaluate(e => e.scrollHeight <= e.clientHeight + 1)).toBe(true);
 await expect(page.locator('.term-dot')).toBeHidden();
 await expect(page.locator('.reveal').first()).toHaveCSS('animation-name', 'none');
 await hydrate(page, '#how');
 await expect(page.locator('[data-stage=hear]')).not.toHaveAttribute('data-state', 'armed');
 await expect(page.locator('[data-stage=hear] [data-after]').last()).toBeVisible();
 await expect(page.locator('#how svg[role="img"]:visible')).toHaveCount(1);
});

test.describe('hero terminal player', () => {
 test('first paint: prompt and blinking cursor, no blank pane', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/?term=at:0');
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'paused');
  await expect(visibleRow(page, 'w', 'a', '~/work/alpha')).toHaveCount(1);
  await expect(page.locator('.term-v[data-v=w] [data-p=a] .cur')).toBeVisible();
  await expect(visibleRow(page, 'w', 'b', 'glen tail -f e27c0de')).toBeVisible();
  const top = await page.locator('.term-win').boundingBox();
  expect(top!.y).toBeLessThan(800);
  await expect(page.locator('.term-lights i')).toHaveCount(3);
  await expect(page.locator('.term-title')).toHaveText('~/work/alpha — zsh');
  await expect(page.locator('#term input[type=checkbox]')).toHaveCount(0);
 });
 test('5300 ms: the user message is in the omp pane, the spinner in the agent pane', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/?term=at:5300');
  await expect(visibleRow(page, 'w', 'b', '❯ user · 12:04')).toBeVisible();
  const spin = page.locator('.term-v[data-v=w] [data-p=a] .tl.on[data-k=spin]');
  await expect(spin).toContainText('waiting for the reply');
  expect('⠁⠉⠙⠚⠒⠂⠲⠴⠤⠄⠠⠦⠖⠐⠓⠋⠈').toContain(await spin.locator('.sp').textContent());
  await expect(page.locator('.term-title')).toHaveText('~/work/alpha — glen');
  await expect(page.locator('.term-tab[data-p=b]')).toHaveAttribute('aria-selected', 'true');
 });
 test('the spinner message follows glen send: sending, starting omp, waiting for the reply', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/?term=at:0');
  const spinAt = Number(await page.locator('.term-v[data-v=w] [data-p=a] .tl[data-k=spin]').getAttribute('data-at'));
  const msgs = (JSON.parse((await page.locator('#term').getAttribute('data-cues'))!) as [number, string, string][]).filter(c => c[1] === 'msg');
  expect(msgs.map(c => c[2])).toEqual(['starting omp', 'waiting for the reply']);
  for (const [at, text] of [[spinAt, 'sending to e27c0de · ja3c9f41'], ...msgs.map(([at, , v]) => [at, v] as const)] as const) {
   await page.goto(`/?term=at:${at + 20}`);
   await expect(page.locator('.term-v[data-v=w] [data-p=a] .tl.on[data-k=spin] .sm')).toHaveText(text);
  }
 });
 test('the reply hops from the omp pane to the agent pane', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/?term=at:0');
  const hop = (JSON.parse((await page.locator('#term').getAttribute('data-cues'))!) as [number, string][]).find(c => c[1] === 'hop')![0];
  await page.goto(`/?term=at:${hop + 120}`);
  await expect(page.locator('.term-dot')).toHaveCSS('opacity', '1');
  const dot = (await page.locator('.term-dot').boundingBox())!;
  const reply = (await page.locator('.term-v[data-v=w] [data-p=b] [data-hop]').boundingBox())!;
  const spin = (await page.locator('.term-v[data-v=w] [data-p=a] .tl.on[data-k=spin] .sp').boundingBox())!;
  expect(dot.x).toBeLessThan(reply.x + 24);
  expect(dot.x).toBeGreaterThan(spin.x);
  expect(dot.y).toBeGreaterThan(spin.y - 8); expect(dot.y).toBeLessThan(reply.y + reply.height);
 });
 for (const [width, variant] of [[1280, 'w'], [390, 'n']] as const) test(`${width}px end frame: real glen output`, async ({ page }) => {
  await page.setViewportSize({ width, height: 900 }); await page.goto('/?term=end');
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'done');
  await expect(visibleRow(page, variant, 'a', /^✓ completed · ja3c9f41 · e27c0de · 41s$/)).toBeVisible();
  await expect(visibleRow(page, variant, 'a', '↩ Added the index. All 12 tests pass. ✓')).toBeVisible();
  await expect(visibleRow(page, variant, 'a', /^handle\s+state\s+title\s+/)).toBeVisible();
  await expect(visibleRow(page, variant, 'a', '3 sessions · 1 live')).toBeVisible();
  await expect(page.locator(`.term-v[data-v=${variant}] [data-p=a] .tl[data-k=spin]`)).toBeHidden();
  const pane = (await page.locator(`.term-v[data-v=${variant}] .term-pane[data-p=a]`).boundingBox())!;
  const first = (await page.locator(`.term-v[data-v=${variant}] [data-p=a] .tl.on`).first().boundingBox())!;
  expect(first.x - pane.x).toBeGreaterThanOrEqual(12);
  if (variant === 'w') { await expect(page.locator('.term-v[data-v=w] .term-pane')).toHaveCount(2); await expect(page.locator('.term-v[data-v=w] .term-pane[data-p=b]')).toBeVisible(); }
  else { await expect(page.locator('.term-v[data-v=n] .term-pane[data-p=b]')).toBeHidden(); await expect(page.locator('.term-tab')).toHaveCount(2); }
 });
 test('plays through, with the omp pane scrolling back', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/?term=fast');
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'playing');
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'done', { timeout: 5000 });
  expect(Number(await page.locator('#term').getAttribute('data-term-t'))).toBeGreaterThan(13000);
  expect(await page.locator('.term-v[data-v=w] [data-p=b] .term-scroll').evaluate(e => e.style.transform)).toMatch(/translateY\(-\d+lh\)/);
 });
 test('Pause freezes the clock; Play resumes; Replay restarts from 0', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/');
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'playing');
  await expect.poll(async () => Number(await page.locator('#term').getAttribute('data-term-t'))).toBeGreaterThan(900);
  const pause = page.getByRole('button', { name: 'Pause demo' });
  await pause.click();
  await expect(pause).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'paused');
  const t = await page.locator('#term').getAttribute('data-term-t');
  await page.waitForTimeout(1000);
  await expect(page.locator('#term')).toHaveAttribute('data-term-t', t!);
  await pause.click();
  await expect(pause).toHaveAttribute('aria-pressed', 'false');
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'playing');
  await page.goto('/?term=end');
  await page.getByRole('button', { name: 'Replay demo' }).click();
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'playing');
  expect(Number(await page.locator('#term').getAttribute('data-term-t'))).toBeLessThan(1500);
  await expect(page.locator('.term-v[data-v=w] [data-p=a] .tl[data-k=spin]')).toBeHidden();
 });
 test('leaving the viewport pauses playback', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/');
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'playing');
  await page.locator('#faq').scrollIntoViewIfNeeded();
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'paused');
  await expect(page.getByRole('button', { name: 'Pause demo' })).toHaveAttribute('aria-pressed', 'false');
 });
 for (const [start, button] of [['end', 'Replay demo'], ['at:3000', 'Pause demo']] as const) test(`?term=${start}: playback started from a seek pauses off screen and in a hidden tab`, async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 }); await page.goto(`/?term=${start}`);
  const term = page.locator('#term');
  await page.getByRole('button', { name: button }).click();
  await expect(term).toHaveAttribute('data-term', 'playing');
  await page.locator('#faq').scrollIntoViewIfNeeded();
  await expect(term).toHaveAttribute('data-term', 'paused');
  await term.scrollIntoViewIfNeeded();
  await expect(term).toHaveAttribute('data-term', 'playing');
  const setHidden = (hidden: boolean) => page.evaluate(hidden => { Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden }); document.dispatchEvent(new Event('visibilitychange')); }, hidden);
  await setHidden(true); await expect(term).toHaveAttribute('data-term', 'paused');
  await setHidden(false); await expect(term).toHaveAttribute('data-term', 'playing');
 });
 test('turning reduced motion on mid-run stops on the final frame', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/');
  const term = page.locator('#term');
  await expect(term).toHaveAttribute('data-term', 'playing');
  // Mid-command: the send line is half typed and the omp pane is still short.
  await expect.poll(async () => Number(await term.getAttribute('data-term-t'))).toBeGreaterThan(1500);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await expect(term).toHaveAttribute('data-term', 'static');
  await expect(term).toHaveAttribute('data-term-t', String(await doneAt(page)));
  await expect(page.locator('.term-ctl')).toBeHidden();
  await expect(page.locator('.term-v[data-v=w] [data-p=a] .tl[data-k=spin]')).toBeHidden();
  const live = await paneText(page);
  // A frame left in the loop would land on t=end and flip the state to done.
  await page.waitForTimeout(400);
  await expect(term).toHaveAttribute('data-term', 'static');
  // Turned off again, the final frame stays and Replay is offered.
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await expect(term).toHaveAttribute('data-term', 'done');
  await expect(page.getByRole('button', { name: 'Replay demo' })).toBeVisible();
  // The frame equals the one a reduced-motion boot renders.
  await page.emulateMedia({ reducedMotion: 'reduce' }); await page.reload();
  await expect(term).toHaveAttribute('data-term', 'static');
  expect(await paneText(page)).toEqual(live);
 });
 test('turning reduced motion on before playback starts keeps the frame static', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); await page.goto('/', { waitUntil: 'domcontentloaded' });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'static');
  // Neither the 600 ms start timer nor the observers may restart it.
  await page.waitForTimeout(1000); await page.locator('#faq').scrollIntoViewIfNeeded(); await page.locator('#term').scrollIntoViewIfNeeded();
  await expect(page.locator('#term')).toHaveAttribute('data-term', 'static');
  for (const row of await page.locator('.term-v[data-v=n] .tl[data-at]:not([data-k=spin])').all()) await expect(row).toBeVisible();
 });
 test('390px: keyboard reaches Pause, Replay, then the tabs, and the tabs switch panes', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); await page.goto('/');
  await page.getByRole('link', { name: 'View on GitHub' }).first().focus();
  await page.keyboard.press('Tab'); await expect(page.getByRole('button', { name: 'Pause demo' })).toBeFocused();
  await page.keyboard.press('Tab'); await expect(page.getByRole('button', { name: 'Replay demo' })).toBeFocused();
  await page.keyboard.press('Tab'); await expect(page.locator('.term-tab[aria-selected=true]')).toBeFocused();
  await page.keyboard.press('ArrowRight');
  await expect(page.locator('.term-tab[data-p=b]')).toBeFocused();
  await expect(page.locator('.term-tab[data-p=b]')).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('.term-v[data-v=n] .term-pane[data-p=b]')).toBeVisible();
  await expect(page.locator('.term-v[data-v=n] .term-pane[data-p=a]')).toBeHidden();
  await expect(page.getByRole('button', { name: 'Pause demo' })).toHaveAttribute('aria-pressed', 'true');
 });
 test('controls have at least 24px hit areas', async ({ page }) => {
  await page.goto('/');
  for (const selector of ['.term-pause', '.term-replay']) {
   const box = await page.locator(selector).boundingBox(); expect(box!.width).toBeGreaterThanOrEqual(24); expect(box!.height).toBeGreaterThanOrEqual(24);
  }
 });
 test('?term=end renders a stable frame', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  const shots: Buffer[] = [];
  for (let i = 0; i < 2; i++) { await page.goto('/?term=end'); await expect(page.locator('#term')).toHaveAttribute('data-term', 'done'); shots.push(await page.locator('#term').screenshot({ animations: 'disabled' })); }
  expect(shots[0]!.equals(shots[1]!)).toBe(true);
 });
 test('dark window edge stands off the panel', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'dark' }); await page.goto('/');
  // Resolve both colours through a canvas, since color-mix() computes to a non-rgb() string.
  const [edge, panel] = await page.evaluate(() => [getComputedStyle(document.querySelector('.term-win')!).borderTopColor, getComputedStyle(document.querySelector('main')!).backgroundColor].map(c => {
   const ctx = document.createElement('canvas').getContext('2d')!; ctx.fillStyle = c; ctx.fillRect(0, 0, 1, 1); return [...ctx.getImageData(0, 0, 1, 1).data.slice(0, 3)];
  }));
  const lum = (c: number[]) => { const [r, g, b] = c.map(v => { const s = v / 255; return s <= .03928 ? s / 12.92 : ((s + .055) / 1.055) ** 2.4; }); return .2126 * r! + .7152 * g! + .0722 * b!; };
  const [a, b] = [lum(edge!), lum(panel!)].sort((x, y) => y - x);
  expect((a! + .05) / (b! + .05)).toBeGreaterThanOrEqual(1.3);
 });
});

test('hero: light display type, baseline row at 1280, three trust checks', async ({ page }) => {
 await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/');
 const h1 = page.getByRole('heading', { level: 1 });
 await expect(h1).toHaveCSS('font-weight', '510');
 const lines = await h1.evaluate(e => Math.round(e.getBoundingClientRect().height / parseFloat(getComputedStyle(e).lineHeight)));
 expect(lines).toBeLessThanOrEqual(3);
 const lead = (await page.locator('.hero-lead-row .body-lg').boundingBox())!, cta = (await page.locator('.hero .cta-primary').boundingBox())!;
 expect(Math.abs(lead.y + lead.height - (cta.y + cta.height))).toBeLessThan(16);
 await expect(page.locator('.hero .cta-primary')).toHaveCSS('border-radius', '10px');
 await expect(page.locator('.hero .cta-primary')).toHaveCSS('font-size', '15px');
 await expect(page.locator('.hero .cta-primary')).toHaveCSS('font-weight', '590');
 await expect(page.locator('.trust-row li')).toHaveCount(3);
 await page.setViewportSize({ width: 390, height: 844 });
 expect(await h1.evaluate(e => Math.round(e.getBoundingClientRect().height / parseFloat(getComputedStyle(e).lineHeight)))).toBeLessThanOrEqual(5);
 const lead390 = (await page.locator('.hero-lead-row .body-lg').boundingBox())!, cta390 = (await page.locator('.hero .cta-primary').boundingBox())!;
 expect(cta390.y).toBeGreaterThan(lead390.y + lead390.height);
});

test('header: sticky glass with the morphing hover fill and a focus ring', async ({ page }) => {
 await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/');
 await page.mouse.wheel(0, 1600); await page.waitForTimeout(300);
 expect((await page.locator('.header-glass').boundingBox())!.y).toBe(0);
 expect(await page.locator('.header-glass').evaluate(e => getComputedStyle(e).backdropFilter)).not.toBe('none');
 const item = page.locator('.desktop-nav .nav-item').first();
 await item.hover();
 await expect(item.locator('.nav-fill')).toHaveCSS('transform', 'none');
 await page.mouse.move(0, 400);
 await item.focus(); await page.keyboard.press('Shift+Tab'); await page.keyboard.press('Tab');
 await expect(item).toHaveCSS('outline-style', 'solid');
 await expect(item.locator('.nav-fill')).toHaveCSS('transform', 'none');
});

test('header: hover and keyboard focus swap the nav labels; reduced motion keeps one', async ({ page }) => {
 await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/');
 const item = page.locator('.desktop-nav .nav-item').first();
 const [rest, swap] = [item.locator('.nav-label').first(), item.locator('.nav-label').nth(1)];
 const swapped = async () => {
  await expect(rest).toHaveCSS('opacity', '0'); await expect(swap).toHaveCSS('opacity', '1'); await expect(swap).toHaveCSS('transform', 'none');
  expect(await rest.evaluate(e => getComputedStyle(e).transform)).not.toBe('none');
 };
 await expect(rest).toHaveCSS('opacity', '1'); await expect(swap).toHaveCSS('opacity', '0');
 await item.hover(); await swapped();
 await page.mouse.move(0, 400); await expect(rest).toHaveCSS('opacity', '1'); await expect(swap).toHaveCSS('opacity', '0');
 await item.focus(); await page.keyboard.press('Shift+Tab'); await page.keyboard.press('Tab'); await swapped();
 await page.emulateMedia({ reducedMotion: 'reduce' }); await item.hover();
 await expect(rest).toHaveCSS('opacity', '1'); await expect(rest).toHaveCSS('transform', 'none'); await expect(swap).toBeHidden();
});

test('sections: mono eyebrow, two-tone h2 at 510, lead, no border dividers', async ({ page }) => {
 await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/');
 for (const [id, n] of [['#how', '01'], ['#install', '02'], ['#agents', '03'], ['#faq', '04']]) {
  const head = page.locator(`${id} .section-head`);
  await expect(head.locator('.eyebrow')).toHaveText(new RegExp(`^${n} / `));
  await expect(head.locator('.eyebrow')).toHaveCSS('font-family', /Geist Mono/);
  const h2 = head.locator('h2');
  await expect(h2).toHaveCSS('font-weight', '510');
  const size = parseFloat(await h2.evaluate(e => getComputedStyle(e).fontSize));
  expect(size).toBeGreaterThanOrEqual(28); expect(size).toBeLessThanOrEqual(48);
  await expect(h2.locator('strong')).toHaveCount(1);
  await expect(head.locator('.section-lead')).toBeVisible();
  await expect(page.locator(id)).toHaveCSS('border-top-width', '0px');
 }
 await expect(page.locator('hr.hairline-fade')).toHaveCount(3);
 await expect(page.locator('#how')).toHaveClass(/band/); await expect(page.locator('#agents')).toHaveClass(/band/);
});

test('how it works: four cards, 2×2 at 1280, each stage ends on its final frame', async ({ page }) => {
 await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/');
 await hydrate(page, '#how');
 const cards = page.locator('.how-card');
 await expect(cards).toHaveCount(4);
 const boxes = await cards.evaluateAll(els => els.map(e => e.getBoundingClientRect()).map(r => ({ x: Math.round(r.x), y: Math.round(r.y) })));
 expect(boxes[0]!.y).toBe(boxes[1]!.y); expect(boxes[0]!.x).toBe(boxes[2]!.x); expect(boxes[2]!.y).toBeGreaterThan(boxes[0]!.y);
 for (const stage of ['find', 'read', 'send', 'hear']) {
  await page.locator(`[data-stage=${stage}]`).scrollIntoViewIfNeeded();
  await expect(page.locator(`[data-stage=${stage}]`)).toHaveAttribute('data-state', 'done', { timeout: 8000 });
 }
 await expect(page.locator('[data-stage=hear] [data-before]')).toBeHidden();
 await expect(page.locator('[data-stage=hear]')).toContainText('↩ Added the index. All 12 tests pass. ✓');
 await expect(page.locator('[data-stage=hear]')).toContainText('✓ completed · ja3c9f41 · e27c0de · 41s');
 expect(await page.locator('[data-stage=hear] [data-before]').textContent()).toContain('waiting for the reply');
 await expect(page.locator('[data-stage=find]')).toContainText('3 sessions · 1 live');
 await page.waitForTimeout(800);
 expect(await page.locator('[data-stage] [data-row], [data-stage] [data-chip], [data-stage] [data-after]').evaluateAll(els => els.every(e => getComputedStyle(e).opacity === '1'))).toBe(true);
});

for (const width of [1280, 390]) test(`${width}px topology: no label overflows its box`, async ({ page }) => {
 await page.setViewportSize({ width, height: 900 }); await page.goto('/'); await hydrate(page, '#how');
 const svg = page.locator('#how svg[role="img"]:visible');
 await expect(svg).toHaveCount(1);
 const overflows = await svg.evaluate(svg => [...svg.querySelectorAll('g > rect')].flatMap(rect => {
  const r = (rect as SVGRectElement).getBBox();
  return [...rect.parentElement!.querySelectorAll('text')].map(text => ({ text: text.textContent, b: (text as SVGTextElement).getBBox() }))
   .filter(({ b }) => b.x < r.x || b.x + b.width > r.x + r.width || b.y < r.y || b.y + b.height > r.y + r.height).map(t => t.text);
 }));
 expect(overflows).toEqual([]);
});

for (const section of ['#install', '#agents']) test(`${section} tabs support automatic keyboard activation`, async ({ page }) => {
 await page.goto('/'); await hydrate(page, section);
 const tabs = page.locator(`${section} [role=tab]`); const selected = page.locator(`${section} [role=tab][aria-selected=true]`);
 const index = await selected.evaluate(el => Array.from(el.parentElement!.children).indexOf(el));
 await selected.focus(); await page.keyboard.press('ArrowRight');
 const next = tabs.nth((index + 1) % await tabs.count()); await expect(next).toHaveAttribute('aria-selected', 'true'); await expect(next).toBeFocused();
 await expect(page.locator(`#${await next.getAttribute('aria-controls')}`)).toBeVisible();
 await expect(page.locator(`${section} [role=tabpanel]:not([hidden])`)).toHaveCount(1);
 await expect(page.locator(`${section} [role=tab][aria-selected=true]`)).toHaveCount(1);
 await expect(next.locator('.tab-pill')).toHaveCount(1);
 await page.keyboard.press('End'); await expect(tabs.last()).toBeFocused(); await expect(tabs.last()).toHaveAttribute('aria-selected', 'true');
 await page.keyboard.press('Home'); await expect(tabs.first()).toBeFocused(); await expect(tabs.first()).toHaveAttribute('aria-selected', 'true');
 await page.keyboard.press('ArrowLeft'); await expect(tabs.last()).toBeFocused(); await expect(tabs.last()).toHaveAttribute('aria-selected', 'true');
});
test('tabs are labelled, with the ink pill on the selected one', async ({ page }) => {
 await page.goto('/'); await hydrate(page, '#install');
 for (const name of ['Script', 'Cargo', 'Binary']) await expect(page.getByRole('tab', { name, exact: true })).toBeVisible();
 const pill = page.locator('#install [role=tab][aria-selected=true] .tab-pill');
 await expect(pill).toHaveCSS('background-color', 'rgb(41, 41, 41)');
 await expect(page.locator('#install [role=tab][aria-selected=true]')).toHaveCSS('color', 'rgb(255, 255, 255)');
});
test('copy writes the exact command and announces success', async ({ page, context }) => {
 await context.grantPermissions(['clipboard-read', 'clipboard-write']); await page.goto('/'); await hydrate(page, '#install');
 const button = page.getByRole('button', { name: 'Copy Cargo command', exact: true });
 await expect(button).toHaveCSS('border-top-width', '0px');
 await expect(button.locator('rect').first()).toHaveAttribute('stroke-dasharray', '16 0');
 await button.click();
 expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(cargo);
 await expect(page.locator('#install-panel-cargo [role=status]')).toHaveText('Copied');
 await page.locator('#install-panel-cargo [role=status]').evaluate(el => {
  const changes: string[] = [];
  new MutationObserver(() => changes.push(el.textContent ?? '')).observe(el, { childList: true, characterData: true, subtree: true });
  Object.assign(window, { copyStatusChanges: changes });
 });
 await page.getByRole('button', { name: 'Copied Cargo command', exact: true }).click();
 await expect(page.locator('#install-panel-cargo [role=status]')).toHaveText('Copied');
 await expect.poll(() => page.evaluate(() => Reflect.get(window, 'copyStatusChanges'))).toEqual(['', 'Copied']);
 await expect(page.getByRole('button', { name: 'Copied Cargo command', exact: true })).toContainText('Copied');
});
for (const failure of ['missing', 'rejected']) test(`copy announces ${failure} Clipboard API failure`, async ({ page }) => {
 await page.addInitScript(failure => Object.defineProperty(navigator, 'clipboard', { configurable: true, value: failure === 'missing' ? undefined : { writeText: () => Promise.reject(new Error('Clipboard denied')) } }), failure);
 await page.goto('/'); await hydrate(page, '#install'); await page.getByRole('button', { name: 'Copy Cargo command', exact: true }).click();
 await expect(page.locator('#install-panel-cargo [role=status]')).toHaveText("Couldn't copy. Select the command and copy it yourself.");
 await expect(page.getByRole('button', { name: 'Copy failed Cargo command', exact: true })).toContainText('Copy failed');
});
test('390px: the cargo command stays on one line and fades out', async ({ page }) => {
 await page.setViewportSize({ width: 390, height: 844 }); await page.goto('/'); await hydrate(page, '#install');
 const pre = page.locator('#install-panel-cargo pre');
 expect(await pre.evaluate(e => e.getBoundingClientRect().height)).toBeLessThan(30);
 await expect(pre).toHaveCSS('mask-image', /linear-gradient/);
});
test('agents: five exit codes with glowing status badges', async ({ page }) => {
 await page.goto('/');
 await expect(page.locator('.exit-row')).toHaveCount(5);
 await expect(page.locator('.exit-badge')).toHaveCount(5);
 await expect(page.locator('.exit-badge i')).toHaveCount(15);
 await expect(page.locator('.exit-row [role=rowheader]')).toHaveText(['0', '10', '6', '7', '11']);
});
test('FAQ: five rows; Enter opens one and the others close', async ({ page }) => {
 await page.goto('/');
 const rows = page.locator('#faq details');
 await expect(rows).toHaveCount(5);
 await rows.nth(1).locator('summary').focus(); await page.keyboard.press('Enter');
 await expect(rows.nth(1)).toHaveAttribute('open', '');
 await rows.nth(3).locator('summary').focus(); await page.keyboard.press('Enter');
 await expect(rows.nth(3)).toHaveAttribute('open', ''); await expect(rows.nth(1)).not.toHaveAttribute('open', '');
 await expect(rows.nth(3).locator('.faq-body')).toContainText('glen wait');
});
test('closing band and footer', async ({ page }) => {
 await page.setViewportSize({ width: 1280, height: 800 }); await page.goto('/');
 await expect(page.locator('.cta-ground > div').first()).toHaveCSS('background-size', '40px 40px, 40px 40px');
 const columns = (sel: string) => page.locator(sel).evaluate(e => getComputedStyle(e).gridTemplateColumns.split(' ').length);
 expect(await columns('.footer-grid')).toBe(5);
 await expect(page.locator('.footer-grid nav h2').first()).toHaveCSS('letter-spacing', '1.68px');
 await page.setViewportSize({ width: 390, height: 844 });
 expect(await columns('.footer-grid')).toBe(2);
});
test('412px first viewport requests no JavaScript file', async ({ page }) => {
 await page.setViewportSize({ width: 412, height: 823 }); const scripts: string[] = [];
 page.on('request', request => { if (request.resourceType() === 'script') scripts.push(request.url()); });
 await page.goto('/'); await page.waitForTimeout(2000); expect(scripts).toEqual([]);
 await expect(page.getByRole('heading', { level: 1 })).toBeVisible(); await expect(page.locator('.term-v[data-v=n] .tl.on').first()).toBeVisible();
});
test('Cargo defaults; unreleased downloads carry release labels', async ({ page }) => {
 await page.goto('/'); await hydrate(page, '#install');
 await expect(page.getByRole('tab', { name: 'Cargo', exact: true })).toHaveAttribute('aria-selected', 'true');
 for (const [name, note] of [['Script', 'The install script ships with the first release (v0.1.0). Use Cargo until then.'], ['Binary', 'Binaries arrive with the v0.1.0 release.']]) {
  await page.getByRole('tab', { name, exact: true }).click();
  await expect(page.locator('#install [role=tabpanel]:not([hidden])')).toContainText(note!);
 }
 await page.getByRole('tab', { name: 'Script', exact: true }).click();
 await expect(page.locator('#install-panel-script button')).toHaveCount(0);
 await expect(page.locator('#install-panel-script code')).toHaveCount(0);
});
test('before the first release the page names no installer URL and no latest release', async ({ page }) => {
 const responses: string[] = [];
 page.on('response', async r => { if (r.url().endsWith('.js')) responses.push(await r.text()); });
 await page.goto('/'); await scrollThrough(page);
 await expect(page.locator('astro-island:not([ssr])')).toHaveCount(4);
 for (const text of [await page.content(), ...responses]) {
  expect(text).not.toContain('raw.githubusercontent.com');
  expect(text).not.toContain('/releases/latest');
 }
});
