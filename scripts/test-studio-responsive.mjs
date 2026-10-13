import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { createServer } from 'vite';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const output = process.env.UI_AUDIT_OUTPUT_DIR || 'J:/CS2ClipStudio/research/benchmarks/redesign/responsive-final';
await mkdir(output, { recursive: true });
const server = await createServer({ cacheDir: 'J:/CS2ClipStudio/cache/responsive-final-vite', server: { host: '127.0.0.1', port: 0, watch: { ignored: ['**/*'] }, hmr: false } });
await server.listen();
const browser = await chromium.launch({ channel: 'chrome', headless: true });
const results = [], failures = [], pageErrors = [], workflowChecks = [];
const physicalSizes = [{ width: 960, height: 640 }, { width: 1366, height: 768 }, { width: 1920, height: 1080 }];
const scales = [1.25, 1.5, 2];
const longName = 'The Unexpectedly Long Player Name With Several Memorable Rounds And Highlights';
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
const baseSource = await readFile(new URL('./test-ui.mjs', import.meta.url), 'utf8');
const baseBegin = baseSource.indexOf('  await page.addInitScript(() => {');
const baseEnd = baseSource.indexOf('  await page.goto(', baseBegin);
const studioSource = await readFile(new URL('./test-studio.mjs', import.meta.url), 'utf8');
const studioBegin = studioSource.indexOf('  await page.addInitScript(()=>{');
const studioEnd = studioSource.indexOf('  await page.goto(', studioBegin);
assert.ok(baseBegin >= 0 && baseEnd > baseBegin && studioBegin >= 0 && studioEnd > studioBegin, 'Stable synthetic fixtures are available');

async function prepare(page, theme, font) {
  const fixture = baseSource.slice(baseBegin, baseEnd).replace("localStorage.setItem('demodesk.appearance', 'dark');", `localStorage.setItem('demodesk.appearance', '${theme}'); localStorage.setItem('demodesk.fontSize', '${font}');`);
  await new AsyncFunction('page', fixture)(page);
  await new AsyncFunction('page', studioSource.slice(studioBegin, studioEnd))(page);
  await page.addInitScript(name => {
    const base = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
      const result = await base(command, args);
      if (command === 'list_demos') return result.map(demo => ({ ...demo, name: name + ' — downloaded match with an especially descriptive file name.dem', mapName: 'Ancient — Championship overtime and a very memorable final round' }));
      if (command === 'get_demo' && result.parsed) {
        result.parsed.info.mapName = 'Ancient — Championship overtime and a very memorable final round';
        result.parsed.info.players.forEach(player => { player.name = name; });
        result.parsed.highlights.forEach(highlight => { highlight.player.name = name; highlight.title = name + ' — unexpected multi-kill and a dramatic late-round finish'; });
      }
      return result;
    };
  }, longName);
  await page.goto(server.resolvedUrls.local[0]);
  await page.getByText('Make every moment').waitFor();
  await page.locator('.home-page .clip-card').first().waitFor();
  await page.evaluate(() => document.fonts.ready);
  assert.equal(await page.evaluate(() => localStorage.getItem('demodesk.appearance')), theme);
  assert.equal(await page.evaluate(() => localStorage.getItem('demodesk.fontSize')), font);
}

async function setLanguage(page, language) {
  await page.evaluate(async language => {
    const { default: i18n } = await import('/src/i18n/index.ts');
    if (language === 'de-stress') i18n.addResourceBundle('de', 'translation', {
      common: { more: 'Weitere Ansichten', teamA: 'Mannschaft Alpha', teamB: 'Mannschaft Bravo' },
      demoView: { reparse: 'Demo erneut analysieren', tabs: { players: 'Spielerstatistiken', charts: 'Ausführliche Diagramme' } },
      studio: { moments: 'Besondere Spielmomente', replay: 'Matchwiedergabe', stats: 'Spielerstatistiken', analysis: 'Gameplay-Auswertung', exports: 'Gespeicherte Videoexporte', moreMatchTabs: 'Weitere Matchansichten', searchMatches: 'Alle heruntergeladenen Spiele durchsuchen', filterMatches: 'Gespeicherte Spiele filtern', clearFilters: 'Alle Suchfilter zurücksetzen', importDemo: 'Demo-Datei importieren', refresh: 'Matchbibliothek aktualisieren' },
    }, true, true);
    await i18n.changeLanguage(language === 'de-stress' ? 'de' : 'en');
  }, language);
  await page.waitForFunction(language => document.documentElement.lang === (language === 'de-stress' ? 'de' : 'en'), language);
}

async function navigate(page, destination) {
  const nav = page.getByRole('navigation', { name: 'Main navigation' });
  const button = nav.getByRole('button', { name: destination, exact: true });
  if (!await button.isVisible()) await page.getByRole('button', { name: 'Menu', exact: true }).click();
  await button.click();
}

async function checkNavigation(page) {
  const menu = page.getByRole('button', { name: 'Menu', exact: true });
  const drawer = await menu.isVisible();
  if (drawer) {
    await menu.click();
    assert.ok(await page.evaluate(() => document.activeElement?.closest('.studio-nav')), 'Drawer initially focuses labeled navigation');
  }
  const nav = page.getByRole('navigation', { name: 'Main navigation' });
  for (const name of ['Home', 'Clips', 'Matches', 'Activity', 'Settings']) {
    const button = nav.getByRole('button', { name, exact: true });
    assert.equal(await button.isVisible(), true, name + ' is visible in the navigation surface');
    const geometry = await button.evaluate(element => { const rect = element.getBoundingClientRect(); return { height: rect.height, left: rect.left, right: rect.right, viewport: innerWidth }; });
    assert.ok(geometry.height >= 40 && geometry.left >= -1 && geometry.right <= geometry.viewport + 1, name + ' has a readable, contained target');
  }
  if (drawer) {
    const count = await page.locator('.studio-nav button:not([disabled])').count();
    for (let i = 0; i < count + 2; i++) {
      await page.keyboard.press('Tab');
      assert.ok(await page.evaluate(() => document.activeElement?.closest('.studio-nav')), 'Drawer retains keyboard focus');
    }
    await page.keyboard.press('Escape');
    assert.equal(await menu.getAttribute('aria-expanded'), 'false');
    assert.equal(await menu.evaluate(element => document.activeElement === element), true, 'Escape returns focus to Menu');
  }
  return drawer;
}

async function geometry(page) {
  return page.evaluate(() => ({ innerWidth, innerHeight, documentWidth: document.documentElement.scrollWidth, documentHeight: document.documentElement.scrollHeight, mainWidth: document.querySelector('.studio-main')?.clientWidth }));
}

async function checkMatchTabs(page, language) {
  await navigate(page, 'Matches');
  await page.locator('.demo-item').first().click();
  const bar = page.locator('.studio-match-tabs');
  await bar.waitFor();
  await bar.scrollIntoViewIfNeeded();
  await page.waitForTimeout(120);
  const labels = await bar.getByRole('tab').allTextContents();
  assert.ok(labels.length >= 3, 'Core match destinations retain visible labels');
  const dimensions = await bar.evaluate(element => ({ width: element.clientWidth, scrollWidth: element.scrollWidth, rect: element.getBoundingClientRect().toJSON(), tabs: [...element.querySelectorAll('[role=tab]')].map(tab => ({ label: tab.textContent.trim(), width: tab.getBoundingClientRect().width, height: tab.getBoundingClientRect().height, left: tab.getBoundingClientRect().left, right: tab.getBoundingClientRect().right, labelRect:tab.querySelector('.demo-tab-label')?.getBoundingClientRect().toJSON(), listRect:tab.closest('.demo-tabs').getBoundingClientRect().toJSON() })) }));
  assert.ok(dimensions.scrollWidth <= dimensions.width + 1, 'Match tab labels and More fit their available width');
  for (const tab of dimensions.tabs) {
    assert.ok(tab.height >= 40 && tab.left >= dimensions.rect.left - 1 && tab.right <= dimensions.rect.right + 1, 'Match tab remains readable and contained: ' + tab.label);
    assert.ok(!tab.labelRect || (tab.labelRect.left >= tab.listRect.left - 1 && tab.labelRect.right <= tab.listRect.right + 1 && tab.labelRect.right <= tab.right + 1), 'Complete tab label is visible: ' + tab.label);
  }
  const more = page.getByRole('button', { name: language === 'de-stress' ? 'Weitere Matchansichten' : 'More match tabs', exact: true });
  const compact = await more.isVisible();
  if (compact) {
    await more.click();
    const analysis = page.getByRole('menuitem', { name: language === 'de-stress' ? 'Gameplay-Auswertung' : 'Analysis', exact: true });
    await analysis.waitFor();
    assert.equal(await analysis.isVisible(), true, 'Overflow destination is available through its text label');
    await page.keyboard.press('Escape');
    assert.equal(await more.evaluate(element => document.activeElement === element), true, 'Match More restores trigger focus');
  }
  return { compact, labels, dimensions };
}

try {
  for (const theme of ['dark', 'light']) for (const font of ['small', 'medium', 'large']) {
    const context = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
    const page = await context.newPage();
    page.setDefaultTimeout(6000);
    page.on('pageerror', error => pageErrors.push({ theme, font, message: error.message }));
    await prepare(page, theme, font);
    for (const language of ['en', 'de-stress']) {
      await setLanguage(page, language);
      for (const nominal of physicalSizes) for (const scale of scales) {
        const viewport = { width: Math.floor(nominal.width / scale), height: Math.floor(nominal.height / scale) };
        const name = `${theme}-${font}-${language}-${nominal.width}x${nominal.height}-${Math.round(scale * 100)}`;
        if(process.env.RESPONSIVE_CASE&&!name.includes(process.env.RESPONSIVE_CASE))continue;
        const result = { name, theme, font, language, nominalWindow: nominal, scalePercent: Math.round(scale * 100), effectiveCssViewport: viewport };
        try {
          await page.setViewportSize(viewport);
          await navigate(page, 'Home');
          result.drawer = await checkNavigation(page);
          result.homeGeometry = await geometry(page);
          assert.ok(result.homeGeometry.documentWidth <= viewport.width + 1, 'Home has no horizontal document overflow');
          if (font === 'large') await page.screenshot({ path: output + '/' + name + '-home.png' });
          result.matchTabs = await checkMatchTabs(page, language);
          result.matchGeometry = await geometry(page);
          assert.ok(result.matchGeometry.documentWidth <= viewport.width + 1, 'Long match names do not overflow the document');
          if (font === 'large') await page.screenshot({ path: output + '/' + name + '-match.png' });
          result.passed = true;
        } catch (error) {
          result.passed = false; result.error = String(error.message || error);
          result.failureGeometry = await page.evaluate(() => ({ viewport: {width:innerWidth,height:innerHeight}, header:document.querySelector('.studio-header')?.getBoundingClientRect().toJSON(), title:document.querySelector('.studio-header-copy')?.getBoundingClientRect().toJSON(), main:document.querySelector('.studio-main')?.getBoundingClientRect().toJSON(), footer:document.querySelector('.studio-status')?.getBoundingClientRect().toJSON() }));
          failures.push({ name, error: result.error });
          console.log(JSON.stringify({name,error:result.error,geometry:result.failureGeometry}));
          await page.screenshot({ path: output + '/' + name + '-failure.png' }).catch(() => {});
          await page.keyboard.press('Escape');
        }
        results.push(result);
        if(results.length%9===0)console.log(JSON.stringify({completed:results.length,failures:failures.length,theme,font,language}));
      }
    }
    await page.setViewportSize({ width: 1366, height: 768 });
    await setLanguage(page, 'en');
    await navigate(page, 'Home');
    await navigate(page, 'Clips');
    await navigate(page, 'Settings');
    await page.getByLabel('Steam ID64', { exact: true }).fill('12345678901234567');
    await page.getByRole('button', { name: 'Go back', exact: true }).click();
    await page.getByRole('alertdialog').waitFor();
    await page.getByRole('button', { name: 'Save and leave', exact: true }).click();
    await page.locator('.clip-gallery-scroll').waitFor();
    assert.equal(await page.evaluate(() => window.studioAuto.settings.steamId), '12345678901234567', 'Save and leave persists the submitted settings');
    await page.getByRole('button', { name: 'Go back', exact: true }).click();
    await page.locator('.home-page').waitFor();
    assert.equal(await page.locator('.studio-settings-host').isVisible(), false, 'Guarded Back consumes the previous history entry');
    workflowChecks.push({ theme, font, guardedBackSaveAndLeave: true });
    await context.close();
  }
} catch (error) {
  failures.push({ name: 'workflow/setup', error: String(error.stack || error) });
} finally {
  const report = { passed: failures.length === 0 && pageErrors.length === 0, method: 'Chromium browser effective CSS viewport checks: nominal width and height divided by 125%, 150%, and 200%. Device scale factor stays 1. This is not Windows OS DPI or browser zoom verification.', localeScope: 'English with synthetic long match/player names; German long-label stress translations injected only into the fixture runtime. German is not a shipped product locale.', inputScope: 'Synthetic parser, jobs, settings and media from test-ui.mjs/test-studio.mjs; no user data, native dialogs, real video, CS2 or GPU operations.', expectedMatrixCases: 108, completedMatrixCases: results.length, results, workflowChecks, failures, pageErrors };
  await writeFile(output + '/responsive-final-result.json', JSON.stringify(report, null, 2));
  await browser.close(); await server.close();
  console.log(JSON.stringify({ passed: report.passed, cases: results.length, failures, pageErrors, report: output + '/responsive-final-result.json' }, null, 2));
  if (!report.passed || !process.env.RESPONSIVE_CASE && results.length !== 108) process.exitCode = 1;
}
