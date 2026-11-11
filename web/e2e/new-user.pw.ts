import type { AgentTranscript } from '@chronoai/cma-sdk';
import { test, expect } from './harness';
import { firstAnswer } from './journey-answer';

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
  await firstAnswer(h, page, ready, identity.subject);
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
  await firstAnswer(h, page, first, h.fixture.accounts.visitor.subject);
  const second = await h.ready(page, (await h.launch(page, 'fresh')).id);
  await firstAnswer(h, page, second, h.fixture.accounts.visitor.subject);
  expect(first.resolution.sandbox.state).toBe('created');
  expect(second.resolution.sandbox.state).toBe('created');
  expect(first.resolution.sandbox.id).not.toBe(second.resolution.sandbox.id);
  expect(h.commands.filter((row) => row.path.endsWith('/launches'))).toHaveLength(2);
  await h.capture(page, 'j1-new-sandbox');
});

test('J2 reuse Sandbox creates a new Workspace each time', async ({ harness: h }) => {
  const page = await h.open('visitor');
  const first = await h.ready(page, (await h.launch(page, 'newWorkspace')).id);
  const original = await firstAnswer(h, page, first, h.fixture.accounts.visitor.subject);
  const second = await h.ready(page, (await h.launch(page, 'newWorkspace')).id);
  await firstAnswer(h, page, second, h.fixture.accounts.visitor.subject);
  expect(second.resolution.sandbox.state).toBe('reused');
  expect(second.resolution.sandbox.id).toBe(first.resolution.sandbox.id);
  expect(second.resolution.workspace.id).not.toBe(first.resolution.workspace.id);
  expect(second.resolution.workspace.state).toBe('created');
  expect(second.resolution.agent.id).not.toBe(first.resolution.agent.id);
  const retained = await h.get<AgentTranscript>(
    page,
    `/api/v1/agents/${first.resolution.agent.id}/transcript`,
  );
  expect(retained.agent_id).toBe(original.agent_id);
  expect(retained.turns).toEqual(original.turns);
  expect(h.commands.filter((row) => row.path.endsWith('/launches'))).toHaveLength(2);
  await h.capture(page, 'j2-new-workspace');
});
