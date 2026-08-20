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
import { open, settle, visit } from './browser.mjs';

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

/**
 * Is the brand mark whole, and does it fit the box that crops it?
 *
 * Two defects shipped here in one day, and neither was visible to jsdom or to
 * any of the checks above.
 *
 * The mark carries its own `prefers-reduced-motion` block. Loaded through an
 * `<img>` that block does not take effect, and the base rules stay frozen —
 * the strokes at `stroke-dashoffset: 100`, the lenses at `opacity: 0` — so a
 * visitor who asked for less motion was shown the arc alone.
 *
 * The crop was then written from `getBoundingClientRect`, which reports fill
 * bounds and excludes stroke, so it sheared six units off an eleven-unit
 * stroke and left the crown of the arc a flat line.
 *
 * Both are geometry and paint, which is why they need a browser to see.
 */
async function brandMark(page, port) {
  const problems = [];
  for (const reduce of [false, true]) {
    await page.emulateMediaFeatures([
      { name: 'prefers-reduced-motion', value: reduce ? 'reduce' : 'no-preference' },
    ]);
    await visit(page, port, 'networkidle0');
    // Long enough for the entrance to finish when it is allowed to run at all.
    await settle(2400);
    const seen = await page.evaluate(() => {
      const wrapper = document.querySelector('.landing__mark');
      if (!wrapper) {
        return { absent: true };
      }
      /*
       * Inline, not an <img>. The mark's own reduced-motion rules only take
       * effect in the page's document — through an <img> they are inert and
       * the mark renders as its arc alone. Nothing below can see inside an
       * image either, so this must fail loudly rather than quietly pass.
       */
      const svg = wrapper.matches('svg') ? wrapper : wrapper.querySelector('svg');
      if (!svg) {
        return { notInline: wrapper.tagName.toLowerCase() };
      }
      const parts = [...svg.querySelectorAll('.cma-arc, .cma-teeth, .cma-lens, .cma-bridge')];
      const invisible = parts
        .filter((el) => {
          const style = getComputedStyle(el);
          // A stroke fully offset draws nothing, and neither does zero opacity.
          const offset = Number.parseFloat(style.strokeDashoffset) || 0;
          return Number(style.opacity) < 0.5 || Math.abs(offset) > 1;
        })
        .map((el) => el.getAttribute('class'));
      const wrap = wrapper.getBoundingClientRect();
      const svgBox = svg.getBoundingClientRect();
      // The viewBox is 100 units wide, so this maps units to CSS pixels. Half
      // the widest stroke is added back because client rects exclude it.
      const unit = svgBox.width / 100;
      const arc = svg.querySelector('.cma-arc').getBoundingClientRect();
      const paintedTop = arc.top - 5.5 * unit;
      const paintedBottom = svgBox.top + 76.5 * unit;
      return {
        missing: false,
        invisible,
        overflowTop: Math.round(wrap.top - paintedTop),
        overflowBottom: Math.round(paintedBottom - wrap.bottom),
        markWidth: Math.round(wrap.width),
      };
    });
    const mode = reduce ? 'reduced motion' : 'normal motion';
    if (seen.absent) {
      problems.push(`${mode}: the landing has no brand mark`);
      continue;
    }
    if (seen.notInline !== undefined) {
      problems.push(
        `${mode}: the mark is a <${seen.notInline}> rather than inline SVG, so its ` +
          `reduced-motion rules cannot take effect`,
      );
      continue;
    }
    if (seen.invisible.length > 0) {
      problems.push(`${mode}: the mark is missing ${seen.invisible.join(', ')}`);
    }
    if (seen.overflowTop > 0) {
      problems.push(`${mode}: the crop cuts ${String(seen.overflowTop)}px off the top of the mark`);
    }
    if (seen.overflowBottom > 0) {
      problems.push(
        `${mode}: the crop cuts ${String(seen.overflowBottom)}px off the bottom of the mark`,
      );
    }
  }
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
    problems.push(...(await brandMark(page, PORT)));
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
