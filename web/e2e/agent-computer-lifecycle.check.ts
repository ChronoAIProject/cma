import { test, expect } from '@playwright/test';
import { cursor, LocalAgentV2 } from './agent-v2.fixture';

test('LOCAL paused native desktop hides controls and never discovers a live connection', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  fixture.boundary.sandbox = {
    ...fixture.boundary.sandbox,
    status: 'paused',
    lifecycle: { pause: { allowed: false, reason: 'not_active' }, revive: { allowed: true } },
  };
  fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
  let probes = 0;
  try {
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.clock.install();
    await page.clock.pauseAt(new Date(Date.now() + 1000));
    await fixture.route(page, baseURL!);
    await page.route(
      /\/api\/v1\/agents\/agt_contract\/(environment|files|browser|desktop)(\/|$)/,
      (route) => {
        probes++;
        return route.fulfill({ status: 503, json: { code: 'unavailable' } });
      },
    );
    await page.goto('/agents/agt_contract');
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    await computer.getByRole('tab', { name: 'Screen', exact: true }).click();
    await expect(computer.getByRole('tabpanel', { name: 'Screen', exact: true })).toContainText(
      'Sandbox paused. Your work is preserved.',
    );
    const desktop = computer.locator('#machine-desktop');
    await expect(desktop).toBeAttached();
    await expect(desktop).toBeHidden();
    await expect(desktop).toHaveAttribute('inert', '');
    await expect(
      computer.getByRole('button', { name: /Reconnect|Take control|Check environment/ }),
    ).toHaveCount(0);
    await page.clock.fastForward(60_000);
    expect(probes).toBe(0);
    expect(fixture.writes).toEqual([]);
    expect(fixture.boundary.writes).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL confirmed pause stops Computer probes, preserves history and resumes only after fresh lifecycle evidence', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  let environmentReads = 0;
  let fileReads = 0;
  let liveScreenReads = 0;
  const active = structuredClone(fixture.boundary.sandbox);
  fixture.boundary.sandbox = {
    ...active,
    status: 'paused',
    lifecycle: {
      pause: { allowed: false, reason: 'not_active' },
      revive: { allowed: true },
    },
  };
  fixture.boundary.agent = {
    ...fixture.boundary.agent,
    sandbox: { ...fixture.boundary.agent.sandbox, status: 'paused' },
  };
  fixture.append({ type: 'turn_started', turn_sequence: 1 });
  fixture.append({
    type: 'tool_started',
    call_id: 'retained',
    title: 'printf paused-proof',
    kind: 'execute',
    status: 'in_progress',
  });
  fixture.append({
    type: 'tool_updated',
    call_id: 'retained',
    status: 'completed',
    title: null,
    output: { text: 'paused-proof', truncated: false },
  });
  fixture.append({ type: 'turn_completed', turn_sequence: 1 });
  const unsupported = {
    state: 'unsupported',
    reason: 'not_supported_by_contract',
    scope: 'none',
    writable: false,
  };
  try {
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.clock.install();
    await page.clock.pauseAt(new Date(Date.now() + 1000));
    await fixture.route(page, baseURL!);
    await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
      route.fulfill({
        json: {
          object: 'cma.agent.event_head',
          schema_version: 1,
          agent_id: 'agt_contract',
          incarnation: 'a'.repeat(64),
          through_cursor: cursor(fixture.items.length),
          active_turn: null,
        },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/environment', (route) => {
      environmentReads++;
      return route.fulfill({
        json: {
          agent_id: 'agt_contract',
          runtime_incarnation: 'b'.repeat(64),
          policy_revision: 1,
          revalidate_at: null,
          files: {
            state: 'available',
            reason: 'authorized_mounted_home',
            scope: 'mounted_home',
            writable: true,
          },
          operation_output: {
            state: 'available',
            reason: 'recorded_output_only',
            scope: 'agent_turn_history',
            writable: false,
          },
          browser: { ...unsupported, state: 'not_enabled', reason: 'browser_disabled' },
          replay: unsupported,
          control: unsupported,
        },
      });
    });
    await page.route('**/api/v1/agents/agt_contract/files/**', (route) => {
      fileReads++;
      return route.fulfill({
        json: { path: new URL(route.request().url()).searchParams.get('path') ?? '', entries: [] },
      });
    });
    await page.route(/\/api\/v1\/agents\/agt_contract\/(browser|desktop)(\/|$)/, (route) => {
      liveScreenReads++;
      return route.fulfill({ status: 503, json: { code: 'unavailable' } });
    });
    await page.goto('/agents/agt_contract');
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    for (const name of ['Files', 'Screen']) {
      await computer.getByRole('tab', { name, exact: true }).click();
      await expect(computer.getByRole('tabpanel', { name, exact: true })).toContainText(
        'Sandbox paused. Your work is preserved.',
      );
      await expect(
        computer.getByRole('button', { name: /Try again|Reconnect|Check environment again/ }),
      ).toHaveCount(0);
      await expect(computer.getByRole('button', { name: 'Open Sandbox to resume' })).toBeVisible();
    }
    await computer.getByRole('tab', { name: 'Operations', exact: true }).click();
    await expect(computer.getByText('paused-proof', { exact: true })).toBeVisible();
    await page.clock.fastForward(60_000);
    expect(environmentReads).toBe(0);
    expect(fileReads).toBe(0);
    expect(liveScreenReads).toBe(0);
    await computer.getByRole('tab', { name: 'Files', exact: true }).click();
    for (const [status, purpose, label] of [
      ['pausing', 'pause', 'Pausing'],
      ['reviving', 'revive', 'Resuming'],
    ] as const) {
      fixture.boundary.sandbox = {
        ...active,
        status,
        lifecycle: {
          ...active.lifecycle!,
          operation:
            purpose === 'pause'
              ? { purpose, phase: 'pause_agents', started_at: fixture.started }
              : { purpose, phase: 'runtime_provisioning', started_at: fixture.started },
        },
      };
      await page.clock.fastForward(5_000);
      await expect(computer.getByRole('tabpanel', { name: 'Files', exact: true })).toContainText(
        label,
      );
      expect(environmentReads).toBe(0);
      expect(fileReads).toBe(0);
    }
    fixture.boundary.sandbox = { ...active, owner: fixture.owner };
    await page.clock.fastForward(5_000);
    await expect.poll(() => environmentReads).toBe(1);
    await expect.poll(() => fileReads).toBe(1);
    await computer.getByRole('tab', { name: 'Screen', exact: true }).click();
    await expect(
      computer.getByText('Live screen is not enabled for this environment.'),
    ).toBeVisible();
    await computer.getByRole('tab', { name: 'Operations', exact: true }).click();
    await expect(computer.getByText('paused-proof', { exact: true })).toBeVisible();
    expect(liveScreenReads).toBe(0);
    expect(fixture.writes).toEqual([]);
    expect(fixture.boundary.writes).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
    expect(fixture.errors).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});
