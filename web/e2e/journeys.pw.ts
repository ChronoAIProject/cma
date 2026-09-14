import type { AgentTranscript } from '@chronoai/cma-sdk';
import { test, expect, type Launch } from './harness';

test('J0 new account returns to the same Trigger and reaches ready after one CMA click', async ({
  harness: h,
}) => {
  const page = await h.open('signup');
  const target = h.fixture.triggers.fresh;
  await page.goto(`${h.config.baseURL}/t/${target.publicId}/${target.placementId}`);
  await expect(page.getByRole('button', { name: 'Create account', exact: true })).toBeEnabled();
  const preview = await h.get<{ technical: { services: string[] } }>(
    page,
    `/api/v1/triggers/public/${target.publicId}`,
  );
  expect(
    preview.technical.services.every((service) => h.fixture.baselineServices.includes(service)),
  ).toBe(true);
  expect(h.commands).toHaveLength(0);
  await h.capture(page, 'j0-landing');
  await h.identity(async () => {
    await page.getByRole('button', { name: 'Create account', exact: true }).click();
    await h.operators.completeSignup(page);
  });
  const launch = await h.currentLaunch(page);
  expect(launch.public_id).toBe(target.publicId);
  expect(launch.placement_id).toBe(target.placementId);
  const identity = await h.get<{
    authenticated: boolean;
    subject: string;
    visitor: boolean;
    capabilities: string[];
  }>(page, '/auth/session');
  expect(identity).toMatchObject({ authenticated: true, visitor: true });
  expect(identity.subject).toBe(h.fixture.accounts.signup.subject);
  expect(Object.values(h.fixture.visitors).map((account) => account.subject)).not.toContain(
    identity.subject,
  );
  expect([...identity.capabilities].sort()).toEqual([
    'delete_own_sandbox',
    'launch_triggers',
    'use_own_sandboxes',
  ]);
  await expect
    .poll(
      async () => ['ready', 'completed'].includes((await h.readLaunch(page, launch.id)).state),
      {
        timeout: 540_000,
        intervals: [1500, 3000, 5000],
      },
    )
    .toBe(true);
  const ready = await h.readLaunch(page, launch.id);
  expect(ready.resolution.agent.id).toBeTruthy();
  await page.waitForURL(`${h.config.baseURL}/agents/${ready.resolution.agent.id}`);
  await expect
    .poll(
      async () => {
        const transcript = await h.get<AgentTranscript>(
          page,
          `/api/v1/agents/${ready.resolution.agent.id}/transcript`,
        );
        return (
          transcript.turns.length === 1 &&
          transcript.turns[0]?.outcome.type === 'completed' &&
          transcript.turns[0].messages.some(
            (message) => message.role === 'assistant' && message.content.trim().length > 0,
          )
        );
      },
      { timeout: 180_000, intervals: [1500, 3000, 5000] },
    )
    .toBe(true);
  await expect(page.locator('.message--assistant .agent-content').first()).toContainText(/\S/);
  await expect(page.getByRole('button', { name: 'Stop response', exact: true })).toHaveCount(0);
  expect(h.commands.filter((row) => row.path.endsWith('/launches'))).toHaveLength(1);
  expect(
    h.commands.filter(
      (row) =>
        row.path.endsWith('/start') ||
        row.path.endsWith('/input') ||
        row.path.endsWith('/agui/run') ||
        /\/responses(?:\/|$)/.test(row.path),
    ),
  ).toHaveLength(0);
  await h.capture(page, 'j0-ready');
});

test('J1 always-new creates distinct visitor-owned Sandboxes', async ({ harness: h }) => {
  const page = await h.open('visitor');
  const admitted = await h.launch(page, 'fresh');
  await expect(page.locator('.public-timeline')).toBeVisible();
  await h.capture(page, 'j1-progress');
  const first = await h.ready(page, admitted.id);
  const second = await h.ready(page, (await h.launch(page, 'fresh')).id);
  expect(first.resolution.sandbox.state).toBe('created');
  expect(second.resolution.sandbox.state).toBe('created');
  expect(first.resolution.sandbox.id).not.toBe(second.resolution.sandbox.id);
  await h.capture(page, 'j1-new-sandbox');
});

test('J2 reuse Sandbox creates a new Workspace each time', async ({ harness: h }) => {
  const page = await h.open('visitor');
  const first = await h.ready(page, (await h.launch(page, 'newWorkspace')).id);
  const second = await h.ready(page, (await h.launch(page, 'newWorkspace')).id);
  expect(second.resolution.sandbox.state).toBe('reused');
  expect(second.resolution.sandbox.id).toBe(first.resolution.sandbox.id);
  expect(second.resolution.workspace.id).not.toBe(first.resolution.workspace.id);
  expect(second.resolution.workspace.state).toBe('created');
  await h.capture(page, 'j2-new-workspace');
});

test('J3 reuse both supports ask resume, ask new and resume-existing', async ({ harness: h }) => {
  const page = await h.open('visitor');
  const first = await h.ready(page, (await h.launch(page, 'ask')).id);
  const target = h.fixture.triggers.ask;
  await page.goto(`${h.config.baseURL}/t/${target.publicId}/${target.placementId}`);
  await page.locator('.public-actions button').click();
  const choice = page.getByRole('dialog', { name: 'Choose your agent' });
  await expect(choice).toBeVisible();
  await h.capture(page, 'j3-choice');
  await choice.getByRole('button', { name: 'Continue where you left off' }).click();
  const resumed = await h.ready(page, (await h.currentLaunch(page)).id);
  expect(resumed.resolution.agent.id).toBe(first.resolution.agent.id);
  await page.goto(`${h.config.baseURL}/t/${target.publicId}/${target.placementId}`);
  await page.locator('.public-actions button').click();
  await choice.getByRole('button', { name: 'Start fresh' }).click();
  const fresh = await h.ready(page, (await h.currentLaunch(page)).id);
  expect(fresh.resolution.workspace.id).toBe(first.resolution.workspace.id);
  expect(fresh.resolution.agent.id).not.toBe(first.resolution.agent.id);
  const existing = await h.ready(page, (await h.launch(page, 'resume')).id);
  const repeated = await h.ready(page, (await h.launch(page, 'resume')).id);
  expect(repeated.resolution.agent.id).toBe(existing.resolution.agent.id);
});

for (const mode of ['oauth', 'api_key', 'device_code'] as const) {
  test(`J4-${mode} G-T2 missing service connection assistant continues via ${mode}`, async ({
    harness: h,
  }) => {
    const page = await h.open('visitor');
    const launch = await h.launchTarget(page, h.fixture.connections[mode]);
    expect(launch.state).toBe('draft');
    await expect(page.locator('.public-connection')).toBeVisible();
    await h.capture(page, `j4-${mode}-connection`);
    await h.identity(async () => {
      const popup = page.waitForEvent('popup');
      await page.getByRole('button', { name: 'Connect', exact: true }).click();
      await h.operators.completeConnection(await popup, mode);
    });
    await h.ready(page, launch.id);
    expect(h.commands.filter((row) => row.path.endsWith('/launches'))).toHaveLength(1);
    expect(
      h.commands.filter((row) => row.path === `/api/v1/launches/${launch.id}/start`),
    ).toHaveLength(1);
    await h.capture(page, `j4-${mode}-ready`);
  });
}

test('J7-retired Trigger cannot admit work', async ({ harness: h }) => {
  const page = await h.open('visitor');
  const target = h.fixture.triggers.retired;
  await page.goto(`${h.config.baseURL}/t/${target.publicId}/${target.placementId}`);
  await expect(page.getByText('This agent is no longer available from this page.')).toBeVisible();
  expect(await page.locator('.public-actions button').count()).toBe(0);
  expect(h.commands).toHaveLength(0);
  await h.capture(page, 'j7-retired');
});

test('J7-paused orchestration freezes a durable Launch without failing it', async ({
  harness: h,
}) => {
  const page = await h.open('visitor');
  const control = { name: 'orchestration_pause' as const, fixtureOwner: h.fixture.fixtureOwner };
  await h.operators.controlFault({ ...control, phase: 'arm' });
  try {
    const launch = await h.launch(page, 'paused');
    await expect(page.getByText('Paused by CMA')).toBeVisible();
    const first = await h.readLaunch(page, launch.id);
    expect([
      'failed_retryable',
      'failed_terminal',
      'cancelled',
      'ready',
      'completed',
    ]).not.toContain(first.state);
    for (let index = 0; index < 3; index++) {
      const response = await h.get<Launch>(page, `/api/v1/launches/${launch.id}`);
      expect(response.state).toBe(first.state);
      expect(response.event_head).toBe(first.event_head);
    }
    await h.capture(page, 'j7-paused');
    await h.operators.controlFault({ ...control, phase: 'release', launchId: launch.id });
    await h.ready(page, launch.id);
  } finally {
    await h.operators.controlFault({ ...control, phase: 'release' });
  }
});
