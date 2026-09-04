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
import { open, visit, settle, ready, CLIPPED_BY } from './browser.mjs';

const PORT = 4319;
const EXPECTED_DEVELOPER_GUIDES = Object.freeze({ typescript: 10, rust: 9 });

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
    model: null,
    reasoning_effort: 'high',
    permission: 'guarded',
    credential: 'delegated',
    size: index === 0 ? undefined : { cpu_millis: 1000, memory_mib: 2048 },
    record_tool_bodies: false,
    skills: [],
  },
  nyxid: { services: [] },
  agent_notes: '',
  mandatory: { packages: [], skills: [] },
  created_at: '2026-08-19T18:45:39Z',
  updated_at: '2026-08-19T18:45:39Z',
  preferred: false,
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
    'https://spec.chrono-ai.fun/cma/1.0/conf/core',
    'https://spec.chrono-ai.fun/cma/1.0/conf/conversation',
    'https://spec.chrono-ai.fun/cma/1.0/conf/usage',
    'https://spec.chrono-ai.fun/cma/1.0/conf/resources',
  ],
  agent: { size: { cpu_millis: 2000, memory_mib: 4096 } },
}));

const CHAT_SESSIONS = [
  {
    id: 'chat_alpha',
    kernel: 'codex',
    status: 'ready',
    cols: 120,
    rows: 32,
    last_seq: 4,
    created_at: '2026-08-20T07:00:00Z',
    conforms_to: ['https://spec.chrono-ai.fun/cma/1.0/conf/conversation'],
    creator: { display_name: 'Ada Lovelace' },
    chat: {
      worktree_name: 'chat-alpha',
      worktree_path: '/work/chat-alpha',
      title: 'Release readiness',
      paused_notice: null,
    },
  },
  {
    id: 'chat_paused',
    kernel: 'codex',
    status: 'idle',
    stop_reason: 'idle_timeout',
    cols: 120,
    rows: 32,
    last_seq: 8,
    created_at: '2026-08-20T06:30:00Z',
    conforms_to: ['https://spec.chrono-ai.fun/cma/1.0/conf/conversation'],
    chat: {
      worktree_name: 'chat-paused',
      worktree_path: '/work/chat-paused',
      title: null,
      paused_notice: 'Your current chat session has been idle for 15min and has been auto-paused.',
    },
  },
];

const CHAT_ALPHA_ITEMS = {
  object: 'list',
  data: [
    {
      id: 'msg_chat_alpha_1',
      type: 'message',
      status: 'completed',
      role: 'assistant',
      content: [
        {
          type: 'output_text',
          text: 'The release checks passed.',
          annotations: [],
          logprobs: [],
        },
      ],
    },
    {
      id: 'tool_chat_alpha_3',
      type: 'cma_chat.tool_activity',
      call_id: 'call-release-checks',
      kind: 'shell',
      target: 'npm test',
      status: 'completed',
      duration_ms: 1450,
      arguments: { text: '{"cwd":"/work/chat-alpha"}', truncated: false },
      result: { text: 'All checks passed', truncated: false },
    },
    {
      id: 'msg_chat_alpha_2',
      type: 'message',
      status: 'completed',
      role: 'user',
      content: [{ type: 'input_text', text: 'Check release readiness.' }],
    },
  ],
  first_id: 'msg_chat_alpha_1',
  last_id: 'msg_chat_alpha_2',
  has_more: false,
};

const BOUNDED_SETUP_FAILURE_OUTPUT = [
  'toolchain installation failed',
  ...Array.from(
    { length: 400 },
    (_, index) => `diagnostic line ${String(index + 1).padStart(3, '0')}`,
  ),
  'final setup diagnostic',
].join('\n');

const CHAT_HOSTS = [
  {
    id: 'chho_0123456789abcdef0123456789abcdef',
    owner: 'subject-chat-owner',
    status: 'paused',
    thread_count: 3,
    runtime_version: 'runtime-v2',
    paused_at: '2026-09-01T01:00:00Z',
    checkpoint_captured_at: '2026-09-01T00:59:58Z',
    last_failure: {
      phase: 'workspace_setup',
      command_index: 1,
      output_excerpt: BOUNDED_SETUP_FAILURE_OUTPUT,
      retryable: true,
      cleanup_pending: false,
      occurred_at: '2026-09-01T01:00:01Z',
    },
    settings_revision: 17,
    created_at: '2026-08-31T23:00:00Z',
  },
  {
    id: 'chho_fedcba9876543210fedcba9876543210',
    owner: 'subject-paused-owner',
    status: 'paused',
    thread_count: 1,
    paused_at: '2026-09-01T01:00:00Z',
    checkpoint_captured_at: '2026-09-01T00:59:58Z',
    settings_revision: 16,
    created_at: '2026-08-31T22:00:00Z',
  },
];

const RESOURCE_READING = {
  cpuMillisUsed: 100,
  cpuMillisLimit: 2000,
  memoryBytesUsed: 435 * 1024 ** 2,
  memoryBytesLimit: 4096 * 1024 ** 2,
  memoryBytesPeak: 512 * 1024 ** 2,
  stallPermille: 0,
  ceilingHits: 0,
  allocationFailures: 0,
  kills: 0,
  receivedBytes: 18 * 1024,
  sentBytes: Math.round(8.9 * 1024),
  receivedBytesPerSecond: 0,
  sentBytesPerSecond: 0,
  intervalMs: 2000,
};

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
    const url = new URL(request.url());
    const path = url.pathname;
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
          'control_any_session',
          'read_user_directory',
          'read_activity_log',
          'read_runtime_inventory',
          'configure_platform',
        ],
      });
    }
    if (path === '/api/v1/agui/capabilities') {
      return void json({
        protocol: 'ag-ui',
        features: { custom: { 'cma:chat': { enabled: true } } },
      });
    }
    if (path === '/api/v1/sessions') {
      const surface = url.searchParams.get('surface');
      return void json({ sessions: surface === 'chat' ? CHAT_SESSIONS : SESSIONS });
    }
    if (path === '/api/v1/conversations/chat_alpha/items') {
      return void json(CHAT_ALPHA_ITEMS);
    }
    if (path === '/api/v1/conversations/chat_paused/items') {
      return void json({
        object: 'list',
        data: [],
        first_id: null,
        last_id: null,
        has_more: false,
      });
    }
    if (path === '/api/v1/kernels') {
      return void json({ kernels: [{ id: 'codex', modes: ['conversation', 'terminal'] }] });
    }
    if (path === '/api/v1/environment-profiles') return void json(PROFILES);
    if (path === '/api/v1/admin/settings') {
      return void json({
        revision: 4,
        settings: {
          chat: {
            enabled: true,
            workspace: {
              repository_url: 'https://github.com/the-omega-institute/trureturing',
              branch: 'dev',
            },
            agent: {
              model: null,
              reasoning_effort: 'high',
              instructions: '',
              record_tool_bodies: true,
            },
            message_prefixes: ['$codex-formal-answer'],
            inactivity: { pause_after_seconds: 900 },
            setup: { commands: [] },
            host: { size: null, max_threads: 10 },
          },
        },
      });
    }
    if (path === '/api/v1/admin/chat/hosts') return void json(CHAT_HOSTS);
    if (path === '/api/v1/agui/events') {
      const threadId = url.searchParams.get('thread_id') ?? 'sesn_alpha';
      const frame = {
        type: 'STATE_DELTA',
        threadId,
        runId: 'run_surface_metrics',
        seq: 5,
        delta: [
          { op: 'replace', path: '/usage/inputTokens', value: 1800 },
          { op: 'replace', path: '/usage/outputTokens', value: 510 },
          { op: 'replace', path: '/usage/contextUsed', value: 42100 },
          { op: 'replace', path: '/usage/contextWindow', value: 200000 },
          { op: 'replace', path: '/resources', value: RESOURCE_READING },
        ],
      };
      const body = `id: 5\nevent: STATE_DELTA\ndata: ${JSON.stringify(frame)}\n\n`;
      return void request.respond({ status: 200, contentType: 'text/event-stream', body });
    }
    if (/\/api\/v1\/sessions\/[^/]+\/resources$/u.test(path)) {
      return void json({ reading: null, observed_at: null, reason: 'no_reading_yet' });
    }
    return void request.continue();
  });
}

/** The capability-gated collection switch replaces both the rail and its create action. */
async function chatSurfaceSwitches(page) {
  const problems = [];
  const initial = await page.evaluate(() => {
    const group = document.querySelector('.sidebar__surfaces');
    const segments = group === null ? [] : [...group.querySelectorAll('button')];
    return {
      role: group?.getAttribute('role'),
      labels: segments.map((segment) => segment.textContent?.trim()),
      pressed: segments.map((segment) => segment.getAttribute('aria-pressed')),
    };
  });
  if (initial.role !== 'group') {
    return ['chat surface: the declared switch is not reachable as a group'];
  }
  if (JSON.stringify(initial.labels) !== JSON.stringify(['Chat', 'Agent'])) {
    problems.push(`chat surface: segments are ${JSON.stringify(initial.labels)}`);
  }
  if (JSON.stringify(initial.pressed) !== JSON.stringify(['false', 'true'])) {
    problems.push(`chat surface: Agent is not the initial selected segment`);
  }

  if (!(await press(page, 'Chat'))) {
    return [...problems, 'chat surface: the Chat segment cannot be pressed'];
  }
  try {
    await page.waitForFunction(
      () =>
        document.querySelector('.rail__label')?.textContent?.trim() === 'Chats' &&
        document.querySelector('.sidebar__new')?.textContent?.trim() === 'New Chat',
      { polling: 'raf', timeout: 5000 },
    );
  } catch {
    problems.push('chat surface: switching did not replace the rail and primary action');
  }
  await page.evaluate(() => {
    document.querySelector('[aria-label="Paused, 1 chat"]')?.click();
  });
  await settle(250);
  const chat = await page.evaluate(() => ({
    subjects: [...document.querySelectorAll('.row-item__subject')].map((row) =>
      row.textContent?.trim(),
    ),
    active: document.querySelector('[aria-label="Active, 1 chat"]') !== null,
    paused: document.querySelector('[aria-label="Paused, 1 chat"]') !== null,
  }));
  if (
    JSON.stringify(chat.subjects.sort()) !==
    JSON.stringify(['Release readiness', 'chat-paused'].sort())
  ) {
    problems.push(`chat surface: row subjects are ${JSON.stringify(chat.subjects)}`);
  }
  if (!chat.active || !chat.paused) {
    problems.push('chat surface: the rail is not grouped into one Active and one Paused chat');
  }

  const selected = await page.evaluate(() => {
    const row = [...document.querySelectorAll('.row-item')].find((candidate) =>
      candidate.textContent?.includes('Release readiness'),
    );
    row?.click();
    return row !== undefined;
  });
  if (!selected) {
    problems.push('chat surface: the durable chat row cannot be opened');
  } else {
    try {
      await ready(page, '.chat-surface[data-surface="chat"]', {
        label: 'the Responses-backed Chat conversation',
      });
      await page.waitForFunction(
        () =>
          document.querySelector('.chat-surface')?.textContent?.includes('release checks passed'),
        { polling: 'raf', timeout: 5000 },
      );
    } catch {
      problems.push('chat surface: durable Responses history did not render');
    }

    await page.setViewport({ width: 1440, height: 900 });
    await settle(250);
    if (!(await press(page, 'Details'))) {
      problems.push('chat surface: Details is not reachable from the header');
    } else {
      await settle(250);
      const details = await page.evaluate(() => {
        const drawer = document.querySelector('.drawer--chat');
        const frame = document.querySelector('.frame');
        return {
          open: frame?.hasAttribute('data-details-collapsed') === false,
          sections:
            drawer === null
              ? []
              : [...drawer.querySelectorAll('.facts__title')].map((title) =>
                  title.textContent?.trim(),
                ),
          text: drawer?.textContent ?? '',
          pathControl:
            drawer?.querySelector('[aria-label="Copy local path /work/chat-alpha"]') !== null,
        };
      });
      if (!details.open) problems.push('chat surface: Details did not open the drawer column');
      if (JSON.stringify(details.sections) !== JSON.stringify(['Worktree', 'Resources'])) {
        problems.push(`chat surface: drawer sections are ${JSON.stringify(details.sections)}`);
      }
      for (const expected of [
        'ready',
        'Started',
        'Ada Lovelace',
        'Release readiness',
        'chat-alpha',
        '/work/chat-alpha',
      ]) {
        if (!details.text.includes(expected)) {
          problems.push(`chat surface: drawer is missing "${expected}"`);
        }
      }
      for (const omitted of [
        'Agent',
        'Skills',
        'Reaches',
        'Trigger repository',
        'Runtime',
        'Identifier',
      ]) {
        if (details.text.includes(omitted)) {
          problems.push(`chat surface: drawer exposes omitted "${omitted}" facts`);
        }
      }
      if (!details.pathControl) {
        problems.push('chat surface: the local worktree path is not copyable');
      }
      await press(page, 'Details');
      await settle(250);
    }

    for (const width of [1440, 390]) {
      await page.setViewport({ width, height: width === 390 ? 844 : 900 });
      await settle(250);
      const reduced = await page.evaluate(() => {
        const surface = document.querySelector('.chat-surface[data-surface="chat"]');
        const actions = [...document.querySelectorAll('.sessionheader .actions button')].map(
          (button) => button.getAttribute('aria-label') ?? button.textContent?.trim(),
        );
        const box = surface?.getBoundingClientRect();
        return {
          mounted: surface !== null,
          visible: box !== undefined && box.width > 0 && box.height > 0,
          actions,
          text: surface?.textContent ?? '',
          hiddenRows:
            surface?.querySelector(
              '.usage, .context, .resources, .resources__pressure, .stage, .stage__mode, .turn--tool, .commandmenu',
            ) !== null,
          thinking: surface?.querySelector('.thinking') !== null,
          finalMessage:
            surface
              ?.querySelector('.turn[data-role="assistant"]')
              ?.textContent?.includes('The release checks passed.') ?? false,
          horizontal: document.documentElement.scrollWidth > window.innerWidth + 1,
        };
      });
      if (!reduced.mounted || !reduced.visible) {
        problems.push(`chat surface at ${String(width)}px: the conversation is not visible`);
      }
      if (JSON.stringify(reduced.actions) !== JSON.stringify(['Details', 'Manage Chat'])) {
        problems.push(
          `chat surface at ${String(width)}px: header actions are ${JSON.stringify(reduced.actions)}`,
        );
      }
      if (reduced.hiddenRows || /\b(?:Usage|Context|Resources)\b/u.test(reduced.text)) {
        problems.push(`chat surface at ${String(width)}px: Agent instrumentation remains in Chat`);
      }
      if (!reduced.thinking || !reduced.finalMessage) {
        problems.push(
          `chat surface at ${String(width)}px: the Thinking fold or final message is absent`,
        );
      }
      if (reduced.horizontal) {
        problems.push(`chat surface at ${String(width)}px: the page overflows horizontally`);
      }
    }
    await page.setViewport({ width: 1440, height: 900 });
    await settle(250);
  }

  const pausedSelected = await page.evaluate(() => {
    const row = [...document.querySelectorAll('.row-item')].find((candidate) =>
      candidate.textContent?.includes('chat-paused'),
    );
    row?.click();
    return row !== undefined;
  });
  if (!pausedSelected) {
    problems.push('chat surface: the paused chat row cannot be opened');
  } else {
    try {
      await page.waitForFunction(
        () => {
          const surface = document.querySelector('.chat-surface[data-surface="chat"]');
          const field = surface?.querySelector('textarea[aria-label="Message"]');
          return (
            surface?.textContent?.includes(
              'Your current chat session has been idle for 15min and has been auto-paused.',
            ) === true &&
            field?.getAttribute('placeholder') ===
              'Send a message to continue with your previous chat session.'
          );
        },
        { polling: 'raf', timeout: 5000 },
      );
    } catch {
      problems.push('chat surface: paused notice and composer copy did not render');
    }

    const pausedView = await page.evaluate(() => {
      const surface = document.querySelector('.chat-surface[data-surface="chat"]');
      const notice = surface?.querySelector('.chat-surface__pause[role="status"]');
      const field = surface?.querySelector('textarea[aria-label="Message"]');
      const visible = (element) => {
        if (!(element instanceof HTMLElement)) return false;
        const box = element.getBoundingClientRect();
        const style = getComputedStyle(element);
        return (
          box.width > 0 &&
          box.height > 0 &&
          style.display !== 'none' &&
          style.visibility !== 'hidden'
        );
      };
      return {
        notice: notice?.textContent?.trim() ?? '',
        noticeVisible: visible(notice),
        placeholder: field?.getAttribute('placeholder') ?? '',
        fieldVisible: visible(field),
        fieldEnabled: field instanceof HTMLTextAreaElement && !field.disabled,
      };
    });
    if (
      pausedView.notice !==
        'Your current chat session has been idle for 15min and has been auto-paused.' ||
      !pausedView.noticeVisible
    ) {
      problems.push(`chat surface: paused notice is not visibly exact (${pausedView.notice})`);
    }
    if (
      pausedView.placeholder !== 'Send a message to continue with your previous chat session.' ||
      !pausedView.fieldVisible ||
      !pausedView.fieldEnabled
    ) {
      problems.push(
        `chat surface: paused composer is not visibly enabled with exact copy (${pausedView.placeholder})`,
      );
    }
  }

  if (!(await press(page, 'Agent'))) {
    return [...problems, 'chat surface: the Agent segment cannot be pressed'];
  }
  try {
    await page.waitForFunction(
      () =>
        document.querySelector('.rail__label')?.textContent?.trim() === 'Sessions' &&
        document.querySelector('.sidebar__new')?.textContent?.trim() === 'New session',
      { polling: 'raf', timeout: 5000 },
    );
  } catch {
    problems.push('chat surface: switching back did not restore Agent');
  }
  return problems;
}

/** The compact session instrumentation remains two rows at every console width. */
async function conversationMetricsStayInTwoRows(page) {
  const problems = [];
  const picked = await page.evaluate(() => {
    const row = document.querySelector('.row-item');
    row?.click();
    return row !== null;
  });
  if (!picked) {
    return ['metrics: no session row is available to open'];
  }
  await settle(300);
  for (const width of [1440, 390]) {
    await page.setViewport({ width, height: 900 });
    try {
      await ready(page, '.resources', { label: 'the live session resource readings' });
    } catch {
      const visible = await page.evaluate(() => document.body.innerText.slice(0, 240));
      problems.push(
        `metrics at ${String(width)}px: the conversation resource row did not mount (${visible})`,
      );
      continue;
    }
    try {
      await page.waitForFunction(
        () => document.querySelector('.resources')?.textContent?.includes('4096 MiB') ?? false,
        { polling: 'raf', timeout: 5000 },
      );
    } catch {
      problems.push(`metrics at ${String(width)}px: the bounded reading never arrived`);
      continue;
    }

    const seen = await page.evaluate(() => {
      const summary = document.querySelector('.conversation__usage-context');
      const usage = summary?.querySelector('.usage');
      const context = summary?.querySelector('.context');
      const resources = document.querySelector('.resources');
      const rows =
        resources === null ? [] : [...resources.querySelectorAll(':scope > .resources__row')];
      if (
        !(summary instanceof HTMLElement) ||
        !(usage instanceof HTMLElement) ||
        !(context instanceof HTMLElement) ||
        !(resources instanceof HTMLElement) ||
        rows.some((row) => !(row instanceof HTMLElement))
      ) {
        return { missing: true };
      }
      const boxes = rows.map((row) => row.getBoundingClientRect());
      const summaryBoxes = [usage, context].map((item) => item.getBoundingClientRect());
      const sameTop = (rectangles) =>
        rectangles.every((rectangle) => Math.abs(rectangle.top - rectangles[0].top) <= 1);
      const noHorizontalOverlap = (rectangles) =>
        rectangles.every(
          (rectangle, index) => index === 0 || rectangle.left >= rectangles[index - 1].right - 1,
        );
      return {
        missing: false,
        summaryGrid: getComputedStyle(summary).display === 'grid',
        resourcesGrid: getComputedStyle(resources).display === 'grid',
        summarySameTop: sameTop(summaryBoxes),
        resourcesSameTop: sameTop(boxes),
        summaryNoOverlap: noHorizontalOverlap(summaryBoxes),
        resourcesNoOverlap: noHorizontalOverlap(boxes),
        rowCount: rows.length,
        text: `${summary.textContent ?? ''} ${resources.textContent ?? ''}`,
        horizontal: document.documentElement.scrollWidth > window.innerWidth + 1,
      };
    });

    if (seen.missing) {
      problems.push(`metrics at ${String(width)}px: the two rows are incomplete`);
      continue;
    }
    if (!seen.summaryGrid || !seen.summarySameTop || !seen.summaryNoOverlap) {
      problems.push(`metrics at ${String(width)}px: tokens and context do not share one fixed row`);
    }
    if (
      !seen.resourcesGrid ||
      !seen.resourcesSameTop ||
      !seen.resourcesNoOverlap ||
      seen.rowCount !== 3
    ) {
      problems.push(
        `metrics at ${String(width)}px: memory, CPU, and network do not share three columns`,
      );
    }
    for (const expected of [
      '2.3K tokens',
      '42.1K / 200K context',
      'Memory · 435 MiB / 4096 MiB',
      'CPU · 100m / 2000m',
      'Network · ↓ 18 KiB · ↑ 8.9 KiB',
    ]) {
      if (!seen.text.includes(expected)) {
        problems.push(`metrics at ${String(width)}px: missing "${expected}"`);
      }
    }
    if (seen.horizontal) {
      problems.push(`metrics at ${String(width)}px: the page overflows horizontally`);
    }
  }
  await page.setViewport({ width: 1440, height: 900 });
  await settle(500);
  return problems;
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

  /*
   * Checked before the list is opened: one Escape closes the menu and the
   * dialog together, so there is no moment afterwards with the dialog still
   * up to look at.
   */
  const authoring = await page.evaluate(() =>
    [...document.querySelectorAll('button')].some(
      (candidate) => candidate.textContent?.trim() === 'New environment profile',
    ),
  );
  if (!authoring) {
    problems.push('picker: no key for authoring a profile beside it');
  }

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
  // Four profiles and the deployment's defaults. Authoring is a key beside the
  // picker rather than an option inside it, so it is not counted here.
  if (seen.options < 5) {
    problems.push(`picker: ${String(seen.options)} options drawn, expected every one of 5`);
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
  const toggle = async () => {
    const button = await page.$('.sidebar__toggle');
    if (button === null) {
      return false;
    }
    await button.click();
    return true;
  };
  const waitForWidth = async (expanded, expected, narrower) => {
    try {
      await page.waitForFunction(
        (nextExpanded, previousWidth, shouldNarrow) => {
          const sidebar = document.querySelector('.sidebar');
          const button = document.querySelector('.sidebar__toggle');
          if (!(sidebar instanceof HTMLElement) || !(button instanceof HTMLButtonElement)) {
            return false;
          }
          const actual = Math.round(sidebar.getBoundingClientRect().width);
          return (
            button.getAttribute('aria-expanded') === String(nextExpanded) &&
            (shouldNarrow ? actual < previousWidth : actual === previousWidth)
          );
        },
        { polling: 'raf', timeout: 5000 },
        expanded,
        expected,
        narrower,
      );
    } catch {
      // The measured assertion below reports the bounded failure precisely.
    }
  };

  const before = await width();
  if (!(await toggle())) {
    return ['no control to close the sidebar'];
  }
  await waitForWidth(false, before, true);
  const closed = await width();
  if (!(await toggle())) {
    return ['no control to reopen the sidebar'];
  }
  await waitForWidth(true, before, false);
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
  // Operations lives in the account menu, so it is opened the way a person
  // opens it rather than by finding a button that no longer sits on the page.
  await page.evaluate(() => {
    document.querySelector('.profile__trigger')?.click();
  });
  await settle();
  const opened = await page.evaluate(() => {
    const row = [...document.querySelectorAll('[role^="menuitem"]')].find((candidate) =>
      /operations/i.test(candidate.textContent ?? ''),
    );
    row?.click();
    return row !== undefined;
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

/** The capability-gated Chat editor is reachable and carries its complete section contract. */
async function chatSettingsDiscoverable(page) {
  await page.evaluate(() => {
    document.querySelector('.profile__trigger')?.click();
  });
  await settle();
  if (!(await press(page, 'Operations'))) {
    return ['chat settings: Operations was not offered to a configure_platform caller'];
  }
  await ready(page, '.operator__tabs', { label: 'the Operations view switch' });
  const opened = await page.evaluate(() => {
    const tab = [...document.querySelectorAll('[role="tab"]')].find(
      (candidate) => candidate.textContent?.trim() === 'Chat',
    );
    tab?.click();
    return tab !== undefined;
  });
  if (!opened) {
    return ['chat settings: no Chat tab under configure_platform'];
  }
  await ready(page, 'form.chat-settings', { label: 'the Chat settings form' });

  const expected = [
    'Workspace',
    'Agent',
    'Messages',
    'Setup',
    'Lifetime',
    'Host resources',
    'Enabled',
  ];
  const seen = await page.evaluate(() => ({
    sections: [...document.querySelectorAll('.chat-settings__section > h3')].map((heading) =>
      heading.textContent?.trim(),
    ),
    repositoryField:
      document.querySelector('form.chat-settings')?.textContent?.includes('Repository URL') ===
      true,
    fixedAccess:
      document
        .querySelector('form.chat-settings')
        ?.textContent?.includes('Chat sessions run with full access (unrestricted).') === true,
  }));
  const problems = [];
  if (JSON.stringify(seen.sections) !== JSON.stringify(expected)) {
    problems.push(`chat settings: section order is ${JSON.stringify(seen.sections)}`);
  }
  if (!seen.repositoryField) {
    problems.push('chat settings: the public repository field is absent');
  }
  if (!seen.fixedAccess) {
    problems.push('chat settings: the fixed unrestricted-access fact is absent');
  }

  // Leave Operations so the remaining signed-in checks begin on the ordinary session surface.
  await page.evaluate(() => {
    document.querySelector('.row-item')?.click();
  });
  await settle();
  return problems;
}

/** Chat-host rows and their inline retirement question fit both supported layouts. */
async function chatHostInventoryFits(page) {
  await page.setViewport({ width: 1440, height: 900 });
  await page.evaluate(() => {
    document.querySelector('.profile__trigger')?.click();
  });
  await settle();
  if (!(await press(page, 'Operations'))) {
    return ['chat hosts: Operations was not offered to a session operator'];
  }
  await ready(page, '.operator__tabs', { label: 'the Operations view switch' });
  const opened = await page.evaluate(() => {
    const tab = [...document.querySelectorAll('[role="tab"]')].find(
      (candidate) => candidate.textContent?.trim() === 'Chat hosts',
    );
    tab?.click();
    return tab !== undefined;
  });
  if (!opened) {
    return ['chat hosts: the inventory tab is absent'];
  }
  await ready(page, '[aria-label="Every chat host"] .record__row', {
    label: 'the chat-host inventory',
  });

  const problems = [];
  for (const width of [1440, 390]) {
    await page.setViewport({ width, height: width === 390 ? 844 : 900 });
    await settle(250);
    if (width === 390 && !(await press(page, 'Retire'))) {
      problems.push('chat hosts at 390px: retirement cannot be opened');
    }
    await settle();
    const seen = await page.evaluate(() => {
      const row = document.querySelector('[aria-label="Every chat host"] .record__row');
      const text = row?.querySelector('.record__text');
      const actions = row?.querySelector('.record__actions');
      const tabs = document.querySelector('.operator__tabs');
      const confirm = row?.querySelector('.confirm');
      const failure = row?.querySelector('.chat-host__failure-output');
      if (row === null || text === null || actions === null || tabs === null) {
        return { missing: true };
      }
      const rowBox = row.getBoundingClientRect();
      const textBox = text.getBoundingClientRect();
      const actionBox = actions.getBoundingClientRect();
      const confirmBox = confirm?.getBoundingClientRect();
      const failureBox = failure?.getBoundingClientRect();
      return {
        missing: false,
        columns: getComputedStyle(row).gridTemplateColumns,
        horizontal: document.documentElement.scrollWidth > window.innerWidth + 1,
        rowOffscreen: rowBox.left < -1 || rowBox.right > window.innerWidth + 1,
        actionsBelow: actionBox.top >= textBox.bottom - 1,
        confirmation: confirm?.textContent?.includes('Retire this chat host?') === true,
        confirmationOffscreen:
          confirmBox !== undefined &&
          (confirmBox.left < rowBox.left - 1 || confirmBox.right > rowBox.right + 1),
        failureBounded:
          failureBox !== undefined &&
          failureBox.height <= 145 &&
          failureBox.left >= rowBox.left - 1 &&
          failureBox.right <= rowBox.right + 1,
        failureScrollable:
          failure !== null &&
          getComputedStyle(failure).overflowY === 'auto' &&
          failure.scrollHeight > failure.clientHeight,
        tabsScrollable:
          getComputedStyle(tabs).overflowX === 'auto' && tabs.scrollWidth > tabs.clientWidth,
        text: row.textContent ?? '',
      };
    });
    if (seen.missing) {
      problems.push(`chat hosts at ${String(width)}px: the first row is incomplete`);
      continue;
    }
    if (!seen.text.includes('subject-chat-owner') || !seen.text.includes('3 threads')) {
      problems.push(`chat hosts at ${String(width)}px: required row facts are absent`);
    }
    if (
      !seen.text.includes('failure workspace_setup, command index 1') ||
      !seen.failureBounded ||
      !seen.failureScrollable
    ) {
      problems.push(`chat hosts at ${String(width)}px: setup failure evidence is not bounded`);
    }
    if (seen.horizontal || seen.rowOffscreen || seen.confirmationOffscreen) {
      problems.push(`chat hosts at ${String(width)}px: the inventory widens the viewport`);
    }
    if (width === 390) {
      if (!seen.actionsBelow || !seen.confirmation) {
        problems.push('chat hosts at 390px: controls do not stack below the row metadata');
      }
      if (!seen.tabsScrollable) {
        problems.push('chat hosts at 390px: the seven-tab strip is not horizontally scrollable');
      }
    } else if (seen.columns.split(' ').length < 3) {
      problems.push('chat hosts at 1440px: the compact row lost its three-column geometry');
    }
  }

  await page.setViewport({ width: 1440, height: 900 });
  await page.evaluate(() => {
    const keep = [...document.querySelectorAll('.confirm button')].find(
      (button) => button.textContent?.trim() === 'Keep it',
    );
    keep?.click();
    document.querySelector('.row-item')?.click();
  });
  await settle();
  return problems;
}

/** Wait for AppFrame's observed width and animated tracks to reach a new viewport. */
async function waitForDeveloperFrame(page, width, phoneControls) {
  try {
    await page.waitForFunction(
      (expectedWidth, expectsPhoneControls) => {
        const frame = document.querySelector('.frame');
        const center = document.querySelector('.frame__center');
        const details = document.querySelector('.frame__details');
        const layout = document.querySelector('.developer__layout');
        const rail = document.querySelector('.developer__rail');
        const picker = document.querySelector('.developer__picker');
        if (
          !(frame instanceof HTMLElement) ||
          !(center instanceof HTMLElement) ||
          !(details instanceof HTMLElement) ||
          !(layout instanceof HTMLElement) ||
          !(rail instanceof HTMLElement) ||
          !(picker instanceof HTMLElement)
        ) {
          return false;
        }

        const frameBox = frame.getBoundingClientRect();
        const centerBox = center.getBoundingClientRect();
        const detailsBox = details.getBoundingClientRect();
        const railDisplay = getComputedStyle(rail).display;
        const pickerDisplay = getComputedStyle(picker).display;
        const controlsReady = expectsPhoneControls
          ? railDisplay === 'none' && pickerDisplay !== 'none'
          : railDisplay !== 'none' && pickerDisplay === 'none';
        return (
          window.innerWidth === expectedWidth &&
          document.documentElement.clientWidth === expectedWidth &&
          Math.abs(frameBox.width - window.innerWidth) <= 1 &&
          controlsReady &&
          frame.getAnimations().every((animation) => animation.playState === 'finished') &&
          centerBox.left >= frameBox.left - 1 &&
          centerBox.right <= frameBox.right + 1 &&
          Math.abs(detailsBox.right - frameBox.right) <= 1
        );
      },
      { polling: 'raf', timeout: 5000 },
      width,
      phoneControls,
    );
    return true;
  } catch {
    return false;
  }
}

/** The bundled Developer view remains readable and leaves no trap at either layout. */
async function developerSurface(page) {
  const problems = [];
  await page.setViewport({ width: 1440, height: 900 });
  await page.evaluate(() => {
    document.querySelector('.profile__trigger')?.click();
  });
  await settle();
  const opened = await page.evaluate(() => {
    const row = [...document.querySelectorAll('[role^="menuitem"]')].find(
      (candidate) => candidate.textContent?.trim() === 'Developer',
    );
    row?.click();
    return row !== undefined;
  });
  if (!opened) {
    return ['developer: the account menu offers no Developer action'];
  }
  await ready(page, '.developer__article h1', { label: 'the bundled Developer guide' });
  await settle(250);

  const desktop = await page.evaluate(() => {
    const surface = document.querySelector('.developer');
    const nav = document.querySelector('.developer__nav');
    const reading = document.querySelector('.developer__reading');
    const preface = document.querySelector('.developer__preface');
    const article = document.querySelector('.developer__article');
    if (
      !(surface instanceof HTMLElement) ||
      !(nav instanceof HTMLElement) ||
      !(reading instanceof HTMLElement) ||
      !(preface instanceof HTMLElement) ||
      !(article instanceof HTMLElement)
    ) {
      return { missing: true };
    }
    const navBox = nav.getBoundingClientRect();
    const readingBox = reading.getBoundingClientRect();
    const prefaceBox = preface.getBoundingClientRect();
    const articleBox = article.getBoundingClientRect();
    return {
      missing: false,
      guides: [...document.querySelectorAll('.developer__document')].map((guide) =>
        guide.textContent?.trim(),
      ),
      heading: article.querySelector('h1')?.textContent?.trim(),
      base: preface.querySelector('code')?.textContent?.trim(),
      origin: window.location.origin,
      readingWidth: readingBox.width,
      overlapsAcross: navBox.right > readingBox.left + 1,
      overlapsDown: prefaceBox.bottom > articleBox.top + 1,
      horizontal: document.documentElement.scrollWidth > window.innerWidth + 1,
    };
  });
  if (desktop.missing) {
    return ['developer: the surface is incomplete'];
  }
  if (desktop.guides.length !== EXPECTED_DEVELOPER_GUIDES.typescript) {
    problems.push(
      `developer: TypeScript draws ${String(desktop.guides.length)} guides, expected ${String(EXPECTED_DEVELOPER_GUIDES.typescript)}`,
    );
  }
  if (!desktop.guides.includes('Sponsorships')) {
    problems.push('developer: TypeScript omits its Sponsorships guide');
  }
  if (desktop.heading !== 'CMA TypeScript SDK') {
    problems.push(`developer: the first bundled heading is ${String(desktop.heading)}`);
  }
  if (desktop.base !== desktop.origin) {
    problems.push(
      `developer: API base ${String(desktop.base)} disagrees with ${String(desktop.origin)}`,
    );
  }
  if (Math.abs(desktop.readingWidth - 748) > 1) {
    problems.push(
      `developer: the desktop reading axis is ${desktop.readingWidth.toFixed(1)}px, expected 748px`,
    );
  }
  if (desktop.overlapsAcross || desktop.overlapsDown) {
    problems.push('developer: navigation, preface and article overlap at desktop width');
  }
  if (desktop.horizontal) {
    problems.push('developer: the desktop surface widens the viewport');
  }

  if (!(await press(page, 'Rust'))) {
    problems.push('developer: the Rust SDK control is absent');
  } else {
    await settle(250);
    const rust = await page.evaluate(() => ({
      heading: document.querySelector('.developer__article h1')?.textContent?.trim() ?? '',
      guides: [...document.querySelectorAll('.developer__document')].map((guide) =>
        guide.textContent?.trim(),
      ),
    }));
    if (rust.heading !== 'CMA Rust SDK') {
      problems.push(`developer: switching to Rust showed "${rust.heading}"`);
    }
    if (rust.guides.length !== EXPECTED_DEVELOPER_GUIDES.rust) {
      problems.push(
        `developer: Rust draws ${String(rust.guides.length)} guides, expected ${String(EXPECTED_DEVELOPER_GUIDES.rust)}`,
      );
    }
    if (rust.guides.includes('Sponsorships')) {
      problems.push('developer: Rust draws the TypeScript-only Sponsorships guide');
    }
  }

  await page.setViewport({ width: 390, height: 844 });
  const phoneLayoutSettled = await waitForDeveloperFrame(page, 390, true);
  const phone = await page.evaluate(() => {
    const frame = document.querySelector('.frame');
    const center = document.querySelector('.frame__center');
    const surface = document.querySelector('.developer');
    const layout = document.querySelector('.developer__layout');
    const rail = document.querySelector('.developer__rail');
    const picker = document.querySelector('.developer__picker');
    if (
      !(frame instanceof HTMLElement) ||
      !(center instanceof HTMLElement) ||
      !(surface instanceof HTMLElement) ||
      !(layout instanceof HTMLElement) ||
      !(rail instanceof HTMLElement) ||
      !(picker instanceof HTMLElement)
    ) {
      return { missing: true };
    }
    const rectangle = (element) => {
      const box = element.getBoundingClientRect();
      return { left: box.left, right: box.right, width: box.width };
    };
    const box = rectangle(layout);
    const wideners = [...document.querySelectorAll('body *')]
      .filter((element) => {
        const candidate = element.getBoundingClientRect();
        if (candidate.width === 0 || candidate.height === 0) return false;
        const style = getComputedStyle(element);
        if (style.display === 'none' || style.visibility === 'hidden') return false;
        return (
          candidate.left < -1 ||
          candidate.right > window.innerWidth + 1 ||
          (element.scrollWidth > element.clientWidth + 1 && style.overflowX === 'visible')
        );
      })
      .slice(0, 8)
      .map((element) => {
        const candidate = rectangle(element);
        return {
          element: `${element.tagName.toLowerCase()}${
            element.getAttribute('class') === null
              ? ''
              : `.${element.getAttribute('class').trim().replaceAll(/\\s+/g, '.')}`
          }`,
          ...candidate,
          clientWidth: element.clientWidth,
          scrollWidth: element.scrollWidth,
        };
      });
    return {
      missing: false,
      rail: getComputedStyle(rail).display,
      picker: getComputedStyle(picker).display,
      offscreen: box.left < -1 || box.right > window.innerWidth + 1,
      horizontal: document.documentElement.scrollWidth > window.innerWidth + 1,
      viewport: window.innerWidth,
      document: {
        clientWidth: document.documentElement.clientWidth,
        scrollWidth: document.documentElement.scrollWidth,
      },
      frame: rectangle(frame),
      grid: getComputedStyle(frame).gridTemplateColumns,
      frameAnimations: frame.getAnimations().map((animation) => animation.playState),
      center: rectangle(center),
      surface: rectangle(surface),
      layout: box,
      wideners,
    };
  });
  if (phone.missing) {
    problems.push('developer: the phone document controls are incomplete');
  } else {
    if (phone.rail !== 'none' || phone.picker === 'none') {
      problems.push('developer: the phone layout does not replace the rail with its selector');
    }
    if (phone.offscreen || phone.horizontal) {
      problems.push(
        'developer: the phone surface widens or leaves the viewport ' +
          JSON.stringify({ settled: phoneLayoutSettled, ...phone }),
      );
    }
  }

  await page.setViewport({ width: 1440, height: 900 });
  await waitForDeveloperFrame(page, 1440, false);
  const picked = await page.evaluate(() => {
    const row = document.querySelector('.row-item');
    row?.click();
    return row !== null;
  });
  if (!picked) {
    problems.push('developer: no session row remains available beside the surface');
  } else {
    await settle(250);
    const stuck = await page.evaluate(() => document.querySelector('.developer') !== null);
    if (stuck) {
      problems.push('developer: choosing a session left Developer on screen');
    }
  }
  return problems;
}

/**
 * Every menu the product opens lands inside the window.
 *
 * Three separate bugs were one bug: a list cut off by a dialog's scrollport, a
 * list 53px past the left edge, a list off the bottom. Each was found by
 * looking at that one surface. This asks the question once, of every trigger
 * there is, so the fourth one is found by the check rather than by somebody
 * using it.
 */
async function everyMenuLands(page) {
  const triggers = await page.evaluate(() =>
    [...document.querySelectorAll('[aria-haspopup="menu"]')].map(
      (trigger, index) => trigger.getAttribute('aria-label') ?? `menu ${String(index)}`,
    ),
  );
  const problems = [];
  for (const [index, name] of triggers.entries()) {
    const opened = await page.evaluate((at) => {
      const trigger = [...document.querySelectorAll('[aria-haspopup="menu"]')][at];
      trigger?.click();
      return trigger !== undefined;
    }, index);
    if (!opened) {
      continue;
    }
    await settle(400);
    const seen = await page.evaluate((clippedBySource) => {
      const clippedBy = eval(`(${clippedBySource})`);
      const menus = [...document.querySelectorAll('.menu')];
      const menu = menus[menus.length - 1];
      if (menu === undefined) {
        return null;
      }
      const box = menu.getBoundingClientRect();
      return {
        left: Math.round(box.left),
        top: Math.round(box.top),
        right: Math.round(box.right),
        bottom: Math.round(box.bottom),
        w: Math.round(box.width),
        h: Math.round(box.height),
        clipped: clippedBy(menu),
        offscreen:
          box.left < 0 ||
          box.top < 0 ||
          box.right > window.innerWidth ||
          box.bottom > window.innerHeight,
      };
    }, CLIPPED_BY);
    if (seen !== null) {
      if (seen.offscreen) {
        problems.push(
          `menu "${name}" lands at ${String(seen.left)},${String(seen.top)} ` +
            `(${String(seen.w)}x${String(seen.h)}) outside the window`,
        );
      }
      if (seen.clipped !== null) {
        problems.push(`menu "${name}" is clipped by .${String(seen.clipped.by)}`);
      }
    }
    await page.keyboard.press('Escape');
    await settle(250);
  }
  return problems;
}

/**
 * The account menu names who is signed in, and no row wraps.
 *
 * The trigger was a 32px avatar whose only text was a screen-reader label, so
 * which account was in use could only be discovered by opening the menu — on a
 * service that spends the owner's own vendor subscription, that is the fact
 * most worth being able to check at a glance.
 */
async function accountMenuReads(page) {
  const problems = [];
  const trigger = await page.evaluate(() => {
    const element = document.querySelector('.profile__trigger');
    return element === null ? null : element.innerText;
  });
  if (trigger === null) {
    return ['account: no trigger in the sidebar foot'];
  }
  if (!trigger.includes('Ada Lovelace')) {
    problems.push('account: the trigger does not name who is signed in');
  }
  if (!trigger.includes('ada@example.test')) {
    problems.push('account: the trigger does not show the account');
  }

  await page.evaluate(() => {
    document.querySelector('.profile__trigger')?.click();
  });
  await settle();
  const rows = await page.evaluate(() => {
    const menus = [...document.querySelectorAll('.menu')];
    const menu = menus[menus.length - 1];
    if (menu === undefined) {
      return null;
    }
    return [...menu.querySelectorAll('[role^="menuitem"]')].map((row) => ({
      text: row.textContent?.trim().slice(0, 40),
      wraps: row.scrollWidth > row.clientWidth + 1,
      icon: row.querySelector('svg') !== null,
    }));
  });
  if (rows === null) {
    return [...problems, 'account: the trigger opens no menu'];
  }
  for (const row of rows) {
    if (row.wraps) {
      problems.push(`account: the row "${String(row.text)}" does not fit on one line`);
    }
    if (!row.icon) {
      problems.push(`account: the row "${String(row.text)}" has no glyph`);
    }
  }
  await page.keyboard.press('Escape');
  await settle(250);
  return problems;
}

/** A deployment-owned size must still look like a machine specification. */
async function profileResourcesAreDiscoverable(page) {
  const problems = [];
  await page.evaluate(() => {
    document.querySelector('.profile__trigger')?.click();
  });
  await settle(250);
  if (!(await press(page, 'Environment profiles'))) {
    return ['profile resources: no Environment profiles action'];
  }
  await ready(page, '.profiles__row', { label: 'an environment profile row' });
  const summary = await page.evaluate(
    () => document.body.textContent?.includes('Session resources: deployment default') ?? false,
  );
  if (!(await press(page, 'Edit'))) {
    return ['profile resources: no Edit action'];
  }
  await ready(page, '.editor__resources', { label: 'the session resource controls' });

  const seen = await page.evaluate((listNamesResources) => {
    const fieldFor = (wanted) => {
      const label = [...document.querySelectorAll('label')].find(
        (candidate) => candidate.textContent?.trim() === wanted,
      );
      return label instanceof HTMLLabelElement && label.htmlFor !== ''
        ? document.getElementById(label.htmlFor)
        : null;
    };
    const group = document.querySelector('.editor__resources');
    const cpu = fieldFor('CPU (millicores)');
    const memory = fieldFor('Memory (MiB)');
    const useDefault = group?.querySelector('input[type="checkbox"]');
    return {
      heading: group?.textContent?.includes('Session resources') ?? false,
      cpu: cpu instanceof HTMLInputElement,
      cpuDisabled: cpu instanceof HTMLInputElement && cpu.disabled,
      memory: memory instanceof HTMLInputElement,
      memoryDisabled: memory instanceof HTMLInputElement && memory.disabled,
      defaultChecked: useDefault instanceof HTMLInputElement && useDefault.checked,
      summary: listNamesResources,
    };
  }, summary);

  if (!seen.summary) problems.push('profile resources: the list does not name the session size');
  if (!seen.heading) problems.push('profile resources: the editor does not name Session resources');
  if (!seen.cpu || !seen.memory) problems.push('profile resources: CPU or memory is absent');
  if (!seen.cpuDisabled || !seen.memoryDisabled || !seen.defaultChecked) {
    problems.push('profile resources: deployment-owned CPU and memory are not visibly disabled');
  }
  await page.keyboard.press('Escape');
  await settle(250);
  return problems;
}

async function main() {
  const { page, close } = await open({ port: PORT });
  try {
    await stub(page);
    await visit(page, PORT, 'domcontentloaded');
    // The signed-in surface, not a duration. See `ready`: on a loaded runner
    // the fixed sleep this replaced lost the race and every check below then
    // measured an empty page and reported what it could not find.
    await ready(page, '.sidebar__new', { label: 'the New session control' });
    await settle(400);

    const problems = [
      ...(await conversationMetricsStayInTwoRows(page)),
      ...(await chatSurfaceSwitches(page)),
      ...(await pickerReachable(page)),
      ...(await sidebarCollapses(page)),
      ...(await selectingLeavesOperations(page)),
      ...(await chatSettingsDiscoverable(page)),
      ...(await chatHostInventoryFits(page)),
      ...(await developerSurface(page)),
      ...(await everyMenuLands(page)),
      ...(await accountMenuReads(page)),
      ...(await profileResourcesAreDiscoverable(page)),
    ];

    if (problems.length > 0) {
      console.error(`signed-in surfaces: ${String(problems.length)} problem(s)`);
      for (const problem of problems) {
        console.error(`  ${problem}`);
      }
      process.exitCode = 1;
      return;
    }
    console.log(
      'signed-in surfaces: the picker, sidebar toggle, Operations and Developer all act, ' +
        'Developer fits both layouts, session metrics keep two rows, every menu lands inside the window, ' +
        'the Chat surface switches rails and stays reduced at desktop and phone widths, ' +
        'Chat settings and host inventory are reachable, and profile resources remain visible',
    );
  } finally {
    await close();
  }
}

await main();
