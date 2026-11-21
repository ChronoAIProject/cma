import test, { before, after } from 'node:test';
import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';
import { localOrigin, paintedDesktop, sampleSummary, waitForDesktop } from './local-create.mjs';

// Explicit dependency paths keep this focused browser check out of the default
// dependency-free scale suite. No server, identity, or application is started.
let browser;
before(async () => {
  const module = process.env.CMA_CREATE_PLAYWRIGHT_MODULE;
  const executablePath = process.env.CMA_CREATE_BROWSER_EXECUTABLE;
  assert.ok(module && executablePath,
    'Set CMA_CREATE_PLAYWRIGHT_MODULE and CMA_CREATE_BROWSER_EXECUTABLE');
  const { chromium } = await import(pathToFileURL(resolve(module)).href);
  browser = await chromium.launch({ executablePath, headless: true });
});
after(async () => { await browser?.close(); });

test('only explicit local origins are eligible', () => {
  for (const value of ['http://127.0.0.1:4315', 'https://am.cma-scale.test:18080', 'http://[::1]:1234'])
    assert.equal(localOrigin(value), value);
  for (const value of ['https://cma.chronoai.ai', 'http://127.0.0.1@external.test',
    'http://secret@127.0.0.1', 'http://127.0.0.1/path', 'http://127.0.0.1/?token=value',
    'http://127.0.0.1/#fragment', 'file:///tmp/local'])
    assert.throws(() => localOrigin(value));
});

test('missing, failed and over-budget attempts cannot become a pass', () => {
  const rows = Array.from({ length: 10 }, (_, index) => ({ variant: 'plain', outcome: 'complete', elapsed_ms: 1000 + index }));
  assert.equal(sampleSummary(rows).desktop.acceptance, 'NOT_PROVEN');
  rows[9] = { variant: 'plain', outcome: 'failed', elapsed_ms: 1 };
  assert.equal(sampleSummary(rows).plain.acceptance, 'NOT_PROVEN');
  assert.equal(sampleSummary(rows).plain.failed, 1);
  rows[9] = { variant: 'plain', outcome: 'complete', elapsed_ms: 5001 };
  assert.equal(sampleSummary(rows).plain.acceptance, 'FAIL');
  rows[9] = { variant: 'plain', outcome: 'complete', elapsed_ms: 1000, cleanup_failure: { code: 'check_failed' } };
  assert.equal(sampleSummary(rows).plain.acceptance, 'NOT_PROVEN');
});

const frame = '<div class="agent-desktop__frame" data-ready style="position:absolute;left:20px;top:20px;width:100px;height:100px"><canvas width="100" height="100"></canvas></div>';
const cases = [
  ['opaque black is a valid observed frame', () => {}, true],
  ['missing canvas', () => document.querySelector('canvas').remove(), false],
  ['transparent canvas', () => document.querySelector('canvas').getContext('2d').clearRect(0, 0, 100, 100), false],
  ['missing product ready marker', () => document.querySelector('div').removeAttribute('data-ready'), false],
  ['display none', () => { document.querySelector('canvas').style.display = 'none'; }, false],
  ['canvas visibility hidden', () => { document.querySelector('canvas').style.visibility = 'hidden'; }, false],
  ['ancestor visibility hidden', () => { document.querySelector('div').style.visibility = 'hidden'; }, false],
  ['canvas opacity zero', () => { document.querySelector('canvas').style.opacity = '0'; }, false],
  ['ancestor opacity zero', () => { document.querySelector('div').style.opacity = '0'; }, false],
  ['ancestor content visibility hidden', () => { document.querySelector('div').style.contentVisibility = 'hidden'; }, false],
  ['outside viewport', () => { document.querySelector('div').style.left = '2000px'; }, false],
  ['center clipped by ancestor', () => {
    Object.assign(document.querySelector('div').style, { width: '10px', height: '10px', overflow: 'hidden' });
  }, false],
  ['opaque overlay', () => {
    document.body.insertAdjacentHTML('beforeend', '<div style="position:fixed;inset:0;background:black;z-index:99"></div>');
  }, false],
  ['opaque pointer-events none loading overlay', () => {
    document.body.insertAdjacentHTML('beforeend', '<div style="position:fixed;inset:0;background:black;z-index:99;pointer-events:none"></div>');
  }, false],
  ['unreadable canvas', () => {
    const context = document.querySelector('canvas').getContext('2d');
    context.getImageData = () => { throw new DOMException('Unreadable', 'SecurityError'); };
  }, false],
];
for (const [name, prepare, expected] of cases) {
  test(`Chrome frame observer: ${name}`, async () => {
    const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
    try {
      await page.setContent(frame);
      await page.evaluate(() => {
        const context = document.querySelector('canvas').getContext('2d');
        context.fillStyle = 'black'; context.fillRect(0, 0, 100, 100);
      });
      await page.evaluate(prepare);
      assert.equal(await page.evaluate(paintedDesktop), expected);
    } finally { await page.close(); }
  });
}

test('Chrome polling completes only after a loading overlay is removed', async () => {
  const page = await browser.newPage();
  try {
    await page.setContent(frame + '<aside style="position:fixed;inset:0;background:white;pointer-events:none"></aside>');
    await page.evaluate(() => {
      document.querySelector('canvas').getContext('2d').fillRect(0, 0, 100, 100);
    });
    assert.equal(await page.evaluate(paintedDesktop), false);
    await page.evaluate(() => setTimeout(() => document.querySelector('aside').remove(), 100));
    assert.equal(await waitForDesktop(page, 5000), true);
    assert.equal(await page.locator('aside').count(), 0);
  } finally { await page.close(); }
});
