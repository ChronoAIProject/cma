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
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { open, ready, visit } from './browser.mjs';

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
  ['--vscode-scrollbar-shadow', "xterm's own, carried with a #000 fallback"],
]);

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

/**
 * Does the surface actually fill the page?
 *
 * `#339` was a property resolving to nothing. This is its sibling: a property
 * that resolves fine and a box that grows anyway, because `flex: 1 1 auto`
 * asks a parent to distribute space and a block parent does not answer. The
 * page still passes every resolution check while occupying a quarter of the
 * window, so resolution is not the question — occupancy is.
 *
 * Checked at both widths §16 specifies a behaviour for, because a surface can
 * fill a desktop and collapse once its breakpoint restacks it.
 */
async function fills(page) {
  const sizes = [
    { label: 'desktop', width: 1440, height: 900 },
    { label: 'phone', width: 390, height: 844 },
  ];
  const problems = [];
  for (const size of sizes) {
    await page.setViewport({ width: size.width, height: size.height });
    const short = await page.evaluate(() => {
      const root = document.getElementById('root');
      const surface = root?.firstElementChild;
      if (!surface) {
        return { missing: true };
      }
      return {
        missing: false,
        surface: Math.round(surface.getBoundingClientRect().height),
        viewport: window.innerHeight,
        className: surface.className,
      };
    });
    if (short.missing) {
      problems.push(`${size.label}: nothing is mounted under #root`);
      continue;
    }
    /*
     * Not an equality: a surface may legitimately sit a little short of the
     * viewport. A surface at half of it has collapsed, which is the failure
     * being described, and no layout in §16 does that deliberately.
     */
    if (short.surface < short.viewport * 0.9) {
      problems.push(
        `${size.label}: .${short.className.split(' ')[0]} fills ` +
          `${String(short.surface)}px of a ${String(short.viewport)}px viewport`,
      );
    }
  }
  await page.setViewport({ width: 1440, height: 900 });
  return problems;
}

/**
 * Does a focused control draw exactly one focus indicator?
 *
 * A surface that relocates the global ring — the terminal prompt does, and
 * §7 permits it — has to cancel what it relocated. Cancelling the wrong
 * property leaves both painted, and the suite cannot see it because jsdom
 * neither focuses nor computes a shadow. The check is structural rather than
 * a list of surfaces: anything that draws its own indicator on `:focus-within`
 * must not contain a focused element still carrying the global one.
 */
async function oneIndicator(page) {
  return page.evaluate(() => {
    const problems = [];
    const focusable = document.querySelectorAll(
      'input, textarea, button, [href], [tabindex]:not([tabindex="-1"])',
    );
    for (const element of focusable) {
      element.focus();
      if (!element.matches(':focus-visible')) {
        continue;
      }
      const own = getComputedStyle(element).boxShadow;
      if (own === 'none') {
        continue;
      }
      // Walk out to whichever ancestor is also drawing one right now.
      for (let parent = element.parentElement; parent; parent = parent.parentElement) {
        const outer = getComputedStyle(parent).boxShadow;
        if (outer !== 'none' && parent.matches(':focus-within')) {
          const name = element.className || element.tagName.toLowerCase();
          problems.push(
            `.${String(name).split(' ')[0]} draws a focus ring inside ` +
              `.${String(parent.className).split(' ')[0]}, which draws its own`,
          );
          break;
        }
      }
    }
    return problems;
  });
}

/**
 * Is the surface shown when nothing works, itself designed?
 *
 * It is reached only when the control plane cannot be answered, so it renders
 * for nobody in development and for everybody at the worst possible moment.
 * `.app-error` had no rule at all and drew at browser defaults — the one
 * screen the restyle never reached, found by accident while the check was
 * still pointed at the wrong page.
 *
 * Driven deliberately here rather than left to a stub failing by chance: the
 * check answers `/auth/session` with a 401 so it can measure the landing, and
 * that same stub is what stops this state ever appearing again.
 */
async function errorSurface(page, port) {
  const problems = [];
  await page.setViewport({ width: 1440, height: 900 });
  await visit(page, port);

  const seen = await page.evaluate(() => {
    const surface = document.querySelector('.app-error');
    if (surface === null) {
      return { missing: true };
    }
    const style = getComputedStyle(surface);
    return {
      missing: false,
      height: Math.round(surface.getBoundingClientRect().height),
      viewport: window.innerHeight,
      background: style.backgroundColor,
      // A paragraph left at browser defaults inherits the UA serif stack.
      face: getComputedStyle(surface.querySelector('p') ?? surface).fontFamily,
    };
  });

  if (seen.missing) {
    return ['error: the unreachable state does not render its own surface'];
  }
  if (seen.height < seen.viewport * 0.9) {
    problems.push(
      `error: .app-error fills ${String(seen.height)}px of a ` +
        `${String(seen.viewport)}px viewport`,
    );
  }
  if (seen.background === 'rgba(0, 0, 0, 0)') {
    problems.push('error: the unreachable state has no ground colour');
  }
  if (/^(Times|serif)/i.test(seen.face)) {
    problems.push(`error: the unreachable state is set in ${seen.face}`);
  }
  return problems;
}

/** The live character must paint real transparent WebGL pixels at every layout and appearance. */
async function characterScene(page, port) {
  const problems = [];
  const colours = new Map();
  const modes = [
    { label: 'desktop light', width: 1440, height: 900, dark: false, reduce: false },
    { label: 'desktop dark', width: 1440, height: 900, dark: true, reduce: false },
    { label: 'phone light', width: 390, height: 844, dark: false, reduce: false },
    { label: 'phone dark', width: 390, height: 844, dark: true, reduce: false },
    { label: 'reduced motion', width: 1440, height: 900, dark: false, reduce: true },
  ];

  for (const mode of modes) {
    await page.setViewport({ width: mode.width, height: mode.height });
    await page.emulateMediaFeatures([
      { name: 'prefers-reduced-motion', value: mode.reduce ? 'reduce' : 'no-preference' },
    ]);
    await visit(page, port, 'networkidle0');
    await ready(page, '.landing__character[data-ready] canvas', {
      label: `${mode.label} character canvas`,
    });
    const seen = await page.evaluate(async (dark) => {
      document.body.toggleAttribute('data-ds-dark-theme', dark);
      const stage = document.querySelector('.landing__character');
      const canvas = stage?.querySelector('canvas');
      const fallback = stage?.querySelector('.landing__character-fallback');
      const word = document.querySelector('.landing__word');
      if (!(stage instanceof HTMLElement) || !(canvas instanceof HTMLCanvasElement)) {
        return { absent: true };
      }

      const gl = canvas.getContext('webgl2');
      if (gl === null) {
        return { noContext: true };
      }

      const width = gl.drawingBufferWidth;
      const height = gl.drawingBufferHeight;
      const pixels = await new Promise((resolve) => {
        // The scene's resize listener queues its render first. Sampling in the
        // following callback reads that frame before the compositor clears a
        // non-preserved WebGL buffer, including under reduced motion where the
        // scene deliberately does not request another frame.
        window.dispatchEvent(new Event('resize'));
        requestAnimationFrame(() => {
          const frame = new Uint8Array(width * height * 4);
          gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, frame);
          resolve(frame);
        });
      });
      let painted = 0;
      let red = 0;
      let green = 0;
      let blue = 0;
      for (let index = 0; index < pixels.length; index += 4) {
        const alpha = pixels[index + 3] ?? 0;
        if (alpha <= 8) continue;
        painted += 1;
        red += pixels[index] ?? 0;
        green += pixels[index + 1] ?? 0;
        blue += pixels[index + 2] ?? 0;
      }
      const stageBox = stage.getBoundingClientRect();
      const canvasBox = canvas.getBoundingClientRect();
      const wordBox = word?.getBoundingClientRect();
      const total = width * height;
      return {
        absent: false,
        paintedRatio: painted / total,
        transparentRatio: (total - painted) / total,
        colour: painted === 0 ? [0, 0, 0] : [red / painted, green / painted, blue / painted],
        stageWidth: Math.round(stageBox.width),
        stageHeight: Math.round(stageBox.height),
        canvasWidth: Math.round(canvasBox.width),
        canvasHeight: Math.round(canvasBox.height),
        overlapsWord: wordBox === undefined ? true : stageBox.bottom > wordBox.top,
        fallbackVisible:
          fallback === null ? true : getComputedStyle(fallback).visibility !== 'hidden',
        motion: stage.hasAttribute('data-motion'),
        disabled: stage instanceof HTMLButtonElement && stage.disabled,
      };
    }, mode.dark);

    if (seen.absent) {
      problems.push(`${mode.label}: the landing has no character canvas`);
      continue;
    }
    if (seen.noContext) {
      problems.push(`${mode.label}: the canvas has no WebGL2 context`);
      continue;
    }
    if (seen.paintedRatio < 0.05) {
      problems.push(
        `${mode.label}: only ${(seen.paintedRatio * 100).toFixed(1)}% of canvas pixels are painted`,
      );
    }
    if (seen.transparentRatio < 0.25) {
      problems.push(`${mode.label}: the character canvas is not meaningfully transparent`);
    }
    if (
      Math.abs(seen.stageWidth - seen.stageHeight) > 1 ||
      seen.stageWidth !== seen.canvasWidth ||
      seen.stageHeight !== seen.canvasHeight
    ) {
      problems.push(
        `${mode.label}: stage ${String(seen.stageWidth)}x${String(seen.stageHeight)} and canvas ` +
          `${String(seen.canvasWidth)}x${String(seen.canvasHeight)} do not share one square`,
      );
    }
    if (seen.overlapsWord) problems.push(`${mode.label}: the character overlaps the CMA wordmark`);
    if (seen.fallbackVisible)
      problems.push(`${mode.label}: the fallback remains over the live scene`);
    if (mode.reduce && seen.motion) {
      problems.push('reduced motion: the character still advertises an animated interaction');
    }
    if (mode.reduce && !seen.disabled) {
      problems.push('reduced motion: the spin control remains enabled');
    }
    if (!mode.reduce && !seen.motion) {
      problems.push(`${mode.label}: the live character lost its motion state`);
    }
    if (!mode.reduce && seen.disabled) {
      problems.push(`${mode.label}: the live character spin control remains disabled`);
    }
    if (!mode.reduce && mode.width === 1440) colours.set(mode.dark ? 'dark' : 'light', seen.colour);
  }

  const light = colours.get('light');
  const dark = colours.get('dark');
  if (light === undefined || dark === undefined) {
    problems.push('character: the desktop appearance colours were not measured');
  } else {
    const distance = Math.hypot(light[0] - dark[0], light[1] - dark[1], light[2] - dark[2]);
    if (distance < 30) problems.push('character: light and dark paint resolve to the same palette');
  }

  await page.setViewport({ width: 1440, height: 900 });
  await page.emulateMediaFeatures([{ name: 'prefers-reduced-motion', value: 'no-preference' }]);
  return problems;
}

async function main() {
  const names = referenced();
  const { page, close } = await open({ port: PORT });

  try {
    /*
     * The preview server has no control plane behind it, so `/auth/session`
     * comes back as the index HTML and the app renders its error state. That
     * would leave every check below measuring a two-line alert rather than a
     * surface. 401 is what a signed-out visitor actually receives in
     * production, so answering with it renders the real landing.
     */
    await page.setRequestInterception(true);
    /*
     * How the control plane answers, for the page being measured. `signedOut`
     * is the landing; `unreachable` is the error surface, which renders for
     * nobody in development and for everybody at the worst possible moment.
     */
    let identity = 'signedOut';
    page.on('request', (request) => {
      if (new URL(request.url()).pathname === '/auth/session') {
        if (identity === 'unreachable') {
          void request.abort('failed');
          return;
        }
        void request.respond({ status: 401, contentType: 'application/json', body: '{}' });
        return;
      }
      void request.continue();
    });

    await visit(page, PORT);

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

    problems.push(...(await fills(page)));
    problems.push(...(await oneIndicator(page)));
    problems.push(...(await characterScene(page, PORT)));
    identity = 'unreachable';
    problems.push(...(await errorSurface(page, PORT)));
    identity = 'signedOut';

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
    await close();
  }
}

await main();
