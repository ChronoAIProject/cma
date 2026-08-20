/**
 * Does the built stylesheet actually produce the values it describes?
 *
 * Every other frontend check here runs under jsdom, which resolves no custom
 * properties at all. That is not a gap in the tests — it is a gap in what
 * they *can* ask, and #339 went through twenty-two green pull requests with
 * the entire product rendering on browser defaults because of it.
 *
 * So this asks the one question jsdom cannot: load the built page in a real
 * browser and check that every custom property the sheets reference resolves
 * to something, in both themes.
 *
 * Run against the production build, not the source. A stylesheet that is
 * correct in `src/` and broken after bundling is still broken.
 */
import { spawn } from 'node:child_process';
import { readdirSync, readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import puppeteer from 'puppeteer-core';

const PORT = 4318;
const DIST = 'dist/assets';

/**
 * Properties that legitimately do not resolve on `body`, with the reason.
 *
 * Named rather than pattern-matched: an exemption list that grows by accident
 * is the same failure as no check at all, and each of these is a decision
 * somebody made rather than a name that happened to be absent.
 */
const EXEMPT = new Map([
  [
    '--canvas-columns',
    'set inline from JavaScript by SessionCanvas; the CSS fallback is deliberate',
  ],
  ['--mark-lit', 'declared on .mark for its own descendants'],
  ['--mark-quiet', 'declared on .mark for its own descendants'],
  ['--vscode-scrollbar-shadow', "xterm's own, carried with a #000 fallback"],
]);

/** Where Chrome is, on a runner or on a developer's machine. */
function chrome() {
  const candidates = [
    process.env.CHROME_PATH,
    '/usr/bin/google-chrome',
    '/usr/bin/chromium-browser',
    '/usr/bin/chromium',
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  ].filter((path) => typeof path === 'string' && path.length > 0);
  const found = candidates.find((path) => existsSync(path));
  if (found === undefined) {
    throw new Error(
      `no Chrome found. Set CHROME_PATH, or install one of: ${candidates.join(', ')}`,
    );
  }
  return found;
}

/** Every custom property the built sheets reference. */
function referenced() {
  const css = readdirSync(DIST)
    .filter((name) => name.endsWith('.css'))
    .map((name) => readFileSync(join(DIST, name), 'utf8'))
    .join('\n');
  if (css.length === 0) {
    throw new Error(`no stylesheets in ${DIST} — run the build first`);
  }
  return [...new Set([...css.matchAll(/var\(\s*(--[\w-]+)/g)].map((match) => match[1]))].sort();
}

async function main() {
  const names = referenced();
  const server = spawn('npx', ['vite', 'preview', '--port', String(PORT)], { stdio: 'ignore' });
  const browser = await puppeteer.launch({
    executablePath: chrome(),
    headless: 'new',
    args: ['--no-sandbox', '--disable-gpu'],
  });

  try {
    const page = await browser.newPage();
    // The preview server takes a moment; retry rather than racing it.
    for (let attempt = 0; ; attempt += 1) {
      try {
        await page.goto(`http://localhost:${String(PORT)}/`, {
          waitUntil: 'networkidle0',
          timeout: 15_000,
        });
        break;
      } catch (error) {
        if (attempt >= 5) {
          throw error;
        }
        await new Promise((resolve) => setTimeout(resolve, 2000));
      }
    }

    const problems = [];
    for (const dark of [false, true]) {
      const unresolved = await page.evaluate(
        (properties, isDark) => {
          document.body.toggleAttribute('data-ds-dark-theme', isDark);
          const style = getComputedStyle(document.body);
          return properties.filter((name) => style.getPropertyValue(name).trim() === '');
        },
        names,
        dark,
      );
      for (const name of unresolved) {
        if (!EXEMPT.has(name)) {
          problems.push(`${dark ? 'dark' : 'light'}: ${name} resolves to nothing`);
        }
      }

      /*
       * The ground itself, because that is what #339 actually broke: a body
       * with no background and no ink, on a page whose every token was
       * nonetheless "defined somewhere".
       */
      const ground = await page.evaluate(() => {
        const style = getComputedStyle(document.body);
        return { background: style.backgroundColor, color: style.color };
      });
      if (ground.background === 'rgba(0, 0, 0, 0)') {
        problems.push(`${dark ? 'dark' : 'light'}: the page has no ground colour`);
      }
      if (ground.color === 'rgb(0, 0, 0)' && dark) {
        problems.push('dark: the page ink is the browser default');
      }
    }

    if (problems.length > 0) {
      console.error(`computed styles: ${String(problems.length)} problem(s)`);
      for (const problem of problems) {
        console.error(`  ${problem}`);
      }
      process.exitCode = 1;
      return;
    }
    console.log(
      `computed styles: ${String(names.length)} properties resolve in both themes ` +
        `(${String(EXEMPT.size)} exempt)`,
    );
  } finally {
    await browser.close();
    server.kill();
  }
}

await main();
