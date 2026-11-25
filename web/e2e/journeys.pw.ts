import { test, expect, type Launch } from './harness';
import { ConnectionJourney } from './connection-journey';
import { connectionModes } from './connection-journey-contract';

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

for (const mode of connectionModes) {
  test(`J4-${mode} G-T2 missing service connection assistant continues via ${mode}`, async ({
    harness: h,
  }) => {
    const journey = new ConnectionJourney(h, mode);
    const page = await h.open('visitor');
    const launch = await h.launchTarget(page, h.fixture.connections[mode]);
    await journey.begin(page, launch);
    await h.capture(page, `j4-${mode}-connection`);
    await h.identity(async () => {
      const popup = page.waitForEvent('popup');
      const result = journey.startResult(page);
      await page
        .getByRole('region', { name: journey.wizardName })
        .getByRole('button', { name: 'Connect all', exact: true })
        .click();
      const [identity] = await Promise.all([popup, result]);
      const verification = journey.verification(page);
      await Promise.all([h.operators.completeConnection(identity, mode), verification]);
    });
    await journey.finish(page, 'valid');
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
