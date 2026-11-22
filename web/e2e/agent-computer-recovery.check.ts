import { expect, test } from '@playwright/test';
import {
  ComputerRecovery,
  operationText,
  type Dependency,
} from './agent-computer-recovery.fixture';

test('LOCAL Computer recovers across independent control-plane outage windows and three re-entries', async ({
  page,
  baseURL,
}, info) => {
  test.setTimeout(90_000);
  const fixture = new ComputerRecovery();
  const computer = fixture.computer(page);
  try {
    await fixture.route(page, baseURL!);
    await page.goto('/agents/agt_contract');
    await fixture.open(page);
    await fixture.recovered(page);

    fixture.phase = 'environment-only';
    fixture.faults.set('environment', 503);
    await fixture.foreground(page);
    await expect(computer).toContainText('We couldn’t check screen availability.');
    for (const [name, subject] of [
      ['Files', 'file'],
      ['Operations', 'operation'],
    ] as const) {
      await computer.getByRole('tab', { name, exact: true }).click();
      await expect(computer).toContainText(`We couldn’t check ${subject} availability.`);
      await expect(computer.getByRole('button', { name: 'New file', exact: true })).toHaveCount(0);
    }
    const environmentReads = fixture.count('environment');
    fixture.faults.delete('environment');
    fixture.phase = 'environment-recovered';
    await fixture.advance(page, 30_000);
    await expect.poll(() => fixture.count('environment')).toBe(environmentReads + 1);
    await fixture.recovered(page);

    fixture.phase = 'identity-only';
    const composer = page.getByRole('textbox', { name: 'Message', exact: true });
    await composer.fill('Unsent outage draft');
    fixture.faults.set('identity', 503);
    await fixture.foreground(page);
    await expect(
      page.getByText('The identity service is temporarily unavailable. Retrying shortly.', {
        exact: true,
      }),
    ).toBeVisible();
    expect(fixture.count('identity', 503)).toBeGreaterThan(0);
    await expect(computer).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'New file', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeDisabled();
    await expect(composer).toHaveValue('Unsent outage draft');
    fixture.faults.delete('identity');
    fixture.phase = 'identity-recovered';
    await fixture.advance(page, 30_000);
    await expect(
      page.getByText('The identity service is temporarily unavailable. Retrying shortly.', {
        exact: true,
      }),
    ).toHaveCount(0);
    await fixture.recovered(page);
    await expect(composer).toHaveValue('Unsent outage draft');

    fixture.phase = 'concurrent-outage';
    const dependencies: Dependency[] = [
      'organizations',
      'workspace',
      'models',
      'transcript',
      'environment',
    ];
    for (const dependency of dependencies) fixture.faults.set(dependency, 503);
    await page.reload();
    for (const dependency of ['organizations', 'workspace', 'transcript'] as const)
      await expect.poll(() => fixture.count(dependency, 503)).toBeGreaterThan(0);
    await expect(page.getByRole('textbox', { name: 'Contents of notes.txt' })).toHaveCount(0);

    fixture.phase = 'workspace-recovered';
    fixture.faults.delete('workspace');
    await fixture.advance(page, 30_000);
    await fixture.open(page);
    await computer.getByRole('tab', { name: 'Files', exact: true }).click();
    await expect(computer).toContainText('We couldn’t check file availability.');
    await page.getByRole('button', { name: /^Agent settings:/ }).click();
    await expect.poll(() => fixture.count('models', 503)).toBeGreaterThan(0);
    await expect(computer.getByText(operationText, { exact: true })).toHaveCount(0);

    fixture.phase = 'environment-recovered-history-still-failed';
    fixture.faults.delete('environment');
    await fixture.advance(page, 30_000);
    await computer.getByRole('button', { name: 'notes.txt', exact: true }).click();
    await expect(computer.getByRole('textbox', { name: 'Contents of notes.txt' })).toBeVisible();
    await expect(computer.getByText(operationText, { exact: true })).toHaveCount(0);
    expect(fixture.faults.has('transcript')).toBe(true);

    fixture.phase = 'all-recovered';
    fixture.faults.clear();
    await fixture.advance(page, 30_000);
    await fixture.recovered(page);
    for (let entry = 1; entry <= 3; entry++) {
      fixture.phase = `re-entry-${entry}`;
      await page.reload();
      await fixture.open(page);
      await fixture.recovered(page);
      await page.getByRole('button', { name: /^Agent settings:/ }).click();
      await expect
        .poll(() =>
          fixture.requests.filter(
            (read) => read.phase === fixture.phase && read.dependency === 'models',
          ),
        )
        .toHaveLength(1);
      for (const dependency of dependencies)
        expect(
          fixture.requests.some(
            (read) =>
              read.phase === fixture.phase && read.dependency === dependency && read.status === 200,
          ),
        ).toBe(true);
    }
    fixture.assertSafe();
  } finally {
    await fixture.finish(page, info);
  }
});
