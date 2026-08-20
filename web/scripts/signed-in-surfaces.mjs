/**
 * Do the signed-in surfaces put things where a person can reach them?
 *
 * `check:styles` asks whether the sheets resolve and whether the signed-out
 * page is occupied. It cannot reach anything behind identity, and that is
 * where the interface actually is — so the defects it found were the ones
 * visible to a visitor, and the ones behind the sign-in went unmeasured.
 *
 * One of those shipped: a picker inside a dialog had its list clipped, showing
 * two of five options with no way to reach the rest. Under `jsdom` the list is
 * present, measurable and fully visible — every assertion about it passed.
 *
 * So this signs in against a stubbed control plane and measures what the
 * built bundle actually draws. It asserts reachability, never appearance: a
 * colour is `check:styles`'s question and a layout is this one's.
 */
import { open, visit, settle, CLIPPED_BY } from './browser.mjs';

const PORT = 4319;

/** A profile list long enough that a clipped menu hides some of it. */
const PROFILES = [
  'own-account-verify',
  'Conversation probe (GitHub-scoped)',
  'Shining',
  'Talos Controller',
].map((name, index) => ({
  id: `envp_${String(index)}`,
  name,
  variables: [],
  packages: [],
  agent: {
    kernel: 'codex',
    mode: 'conversation',
    reasoning_effort: 'high',
    permission: 'guarded',
    credential: 'delegated',
  },
  created_at: '2026-08-19T18:45:39Z',
  updated_at: '2026-08-19T18:45:39Z',
}));

/**
 * Answer as a control plane would for a signed-in owner with no sessions.
 *
 * No sessions on purpose: this is about chrome — the dialog and the account
 * menu — and a fleet would only add surfaces that are not being asked about.
 */
async function stub(page) {
  await page.setRequestInterception(true);
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    const json = (body) =>
      request.respond({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });

    if (path === '/auth/session') {
      return void json({
        authenticated: true,
        subject: 'sub_check',
        display_name: 'Ada Lovelace',
        email: 'ada@example.test',
      });
    }
    if (path === '/api/v1/sessions') return void json({ sessions: [] });
    if (path === '/api/v1/kernels') {
      return void json({ kernels: [{ id: 'codex', modes: ['conversation', 'terminal'] }] });
    }
    if (path === '/api/v1/environment-profiles') return void json({ profiles: PROFILES });
    if (path.endsWith('/events')) {
      return void request.respond({ status: 200, contentType: 'text/event-stream', body: ':\n\n' });
    }
    return void request.continue();
  });
}

/** Click the first button whose visible text is exactly `text`. */
function press(page, text) {
  return page.evaluate((wanted) => {
    const button = [...document.querySelectorAll('button')].find(
      (candidate) => candidate.textContent?.trim() === wanted,
    );
    button?.click();
    return button !== undefined;
  }, text);
}

/**
 * A picker's list must be reachable, wherever the picker happens to sit.
 *
 * The profile picker lives inside `.modal__body`, which scrolls so a long form
 * can, and an in-place menu is `position: absolute` inside its anchor — so the
 * scrollport cut the list off 119px short and three of five options could not
 * be reached at all.
 */
async function pickerReachable(page) {
  const problems = [];
  if (!(await press(page, 'New session'))) {
    return ['no New session control on the signed-in surface'];
  }
  await settle();
  await page.evaluate(() => {
    document.querySelector('.picker__anchor')?.click();
  });
  await settle();

  const seen = await page.evaluate((clippedBySource) => {
    const clippedBy = eval(`(${clippedBySource})`);
    const menu = document.querySelector('.menu');
    if (menu === null) {
      return { missing: true };
    }
    const box = menu.getBoundingClientRect();
    return {
      missing: false,
      clipped: clippedBy(menu),
      offscreen:
        box.bottom > window.innerHeight ||
        box.top < 0 ||
        box.right > window.innerWidth ||
        box.left < 0,
      options: menu.querySelectorAll('[role^="menuitem"]').length,
    };
  }, CLIPPED_BY);

  if (seen.missing) {
    problems.push('the profile picker opens no list');
    return problems;
  }
  if (seen.clipped !== null) {
    problems.push(
      `picker: the list is clipped by .${String(seen.clipped.by)} (overflow: ${String(seen.clipped.overflow)})`,
    );
  }
  if (seen.offscreen) {
    problems.push('picker: the list falls outside the window');
  }
  // Four profiles, the deployment's defaults, and the authoring entry.
  if (seen.options < 6) {
    problems.push(`picker: ${String(seen.options)} options drawn, expected every one of 6`);
  }
  await page.keyboard.press('Escape');
  await settle(300);
  await page.keyboard.press('Escape');
  await settle(300);
  return problems;
}

async function main() {
  const { page, close } = await open({ port: PORT });
  try {
    await stub(page);
    await visit(page, PORT);
    await settle(1500);

    const problems = await pickerReachable(page);

    if (problems.length > 0) {
      console.error(`signed-in surfaces: ${String(problems.length)} problem(s)`);
      for (const problem of problems) {
        console.error(`  ${problem}`);
      }
      process.exitCode = 1;
      return;
    }
    console.log('signed-in surfaces: every option in the dialog picker is reachable');
  } finally {
    await close();
  }
}

await main();
