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

/** Two sessions, so the rail has rows to choose between. */
const SESSIONS = ['sesn_alpha', 'sesn_beta'].map((id, index) => ({
  id,
  kernel: 'codex',
  status: 'ready',
  cols: 120,
  rows: 32,
  last_seq: 4,
  created_at: `2026-08-20T0${String(6 - index)}:00:00Z`,
  conforms_to: [
    'https://spec.chrono-ai.fun/chrono-bot/1.0/conf/core',
    'https://spec.chrono-ai.fun/chrono-bot/1.0/conf/conversation',
  ],
}));

/**
 * Answer as a control plane would for a signed-in owner who may also operate.
 *
 * The operator capability is granted deliberately: without it the view switch
 * is not offered, and the check that selecting a session leaves Operations
 * would pass by never running — which is the shape of a test that tests
 * nothing.
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
        capabilities: [
          'create_session',
          'use_own_sessions',
          'manage_own_environment_profiles',
          'read_all_sessions',
          'read_user_directory',
        ],
      });
    }
    if (path === '/api/v1/sessions') return void json({ sessions: SESSIONS });
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

/**
 * Does closing the sidebar close the sidebar?
 *
 * It used to set a class on the panel and nothing else. The frame owns the
 * column widths and had no idea the toggle existed, so the sidebar re-laid its
 * own contents into icon mode inside a track that was still 280px wide — the
 * state changed, the layout did not, and the control read as broken.
 *
 * Measured as a rendered width. Asserting the class is what would have passed
 * throughout.
 */
async function sidebarCollapses(page) {
  const width = () =>
    page.evaluate(() =>
      Math.round(document.querySelector('.sidebar')?.getBoundingClientRect().width ?? 0),
    );
  const toggle = () =>
    page.evaluate(() => {
      const button = [...document.querySelectorAll('.sidebar button')].find((candidate) =>
        candidate.hasAttribute('aria-expanded'),
      );
      button?.click();
      return button !== undefined;
    });

  const before = await width();
  if (!(await toggle())) {
    return ['no control to close the sidebar'];
  }
  await settle();
  const closed = await width();
  await toggle();
  await settle();
  const reopened = await width();

  const problems = [];
  if (closed >= before) {
    problems.push(`sidebar: closing left it at ${String(closed)}px, from ${String(before)}px`);
  }
  if (reopened !== before) {
    problems.push(
      `sidebar: reopening gave ${String(reopened)}px, not the ${String(before)}px it had`,
    );
  }
  return problems;
}

/**
 * Choosing a session leaves whatever else was showing.
 *
 * With Operations open, picking a session from the rail changed which session
 * was active and kept the operator panel on screen. The command bar did this
 * correctly and the rail did not, so the surface people use was the broken one.
 *
 * Driven only where the operator panel is reachable: a caller without the
 * capability is not offered it, and that is not a failure.
 */
async function selectingLeavesOperations(page) {
  const opened = await page.evaluate(() => {
    const button = [...document.querySelectorAll('button')].find((candidate) =>
      /operations/i.test(candidate.textContent ?? ''),
    );
    button?.click();
    return button !== undefined;
  });
  if (!opened) {
    return ['operations: the view switch was not offered to a caller who may operate'];
  }
  await settle();
  const picked = await page.evaluate(() => {
    const row = document.querySelector('.row-item');
    row?.click();
    return row !== null;
  });
  if (!picked) {
    return ['no session row to choose while Operations was open'];
  }
  await settle();
  const stuck = await page.evaluate(() => document.querySelector('[class*="operator"]') !== null);
  return stuck ? ['operations: choosing a session left the operator panel on screen'] : [];
}

async function main() {
  const { page, close } = await open({ port: PORT });
  try {
    await stub(page);
    await visit(page, PORT, 'domcontentloaded');
    await settle(1500);

    const problems = [
      ...(await pickerReachable(page)),
      ...(await sidebarCollapses(page)),
      ...(await selectingLeavesOperations(page)),
    ];

    if (problems.length > 0) {
      console.error(`signed-in surfaces: ${String(problems.length)} problem(s)`);
      for (const problem of problems) {
        console.error(`  ${problem}`);
      }
      process.exitCode = 1;
      return;
    }
    console.log('signed-in surfaces: the picker, the sidebar toggle and the view switch all act');
  } finally {
    await close();
  }
}

await main();
