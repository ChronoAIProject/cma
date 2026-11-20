import { expect, test } from '@playwright/test';
import { ComputerRecovery, fileText, operationText } from './agent-computer-recovery.fixture';

test('LOCAL environment reads coalesce, honor Retry-After and stop while hidden or closed', async ({
  page,
  baseURL,
}, info) => {
  const fixture = new ComputerRecovery();
  try {
    await fixture.route(page, baseURL!);
    await page.goto('/agents/agt_contract');
    await fixture.open(page);
    await fixture.recovered(page);
    const before = fixture.count('environment');
    const release = fixture.hold('environment');
    await fixture.foreground(page);
    await expect.poll(() => fixture.count('environment')).toBe(before + 1);
    await fixture.foreground(page);
    expect(fixture.count('environment')).toBe(before + 1);
    release();
    await expect(fixture.computer(page)).toContainText('Live screen is not supported');

    fixture.faults.set('environment', 429);
    await fixture.advance(page, 30_000);
    await expect(fixture.computer(page)).toContainText('We couldn’t check screen availability.');
    const limited = fixture.count('environment');
    fixture.faults.delete('environment');
    await fixture.foreground(page);
    await fixture.advance(page, 59_999);
    expect(fixture.count('environment')).toBe(limited);
    await fixture.advance(page, 1001);
    await expect.poll(() => fixture.count('environment')).toBe(limited + 1);
    await fixture.recovered(page);

    await fixture.hidden(page, true);
    const hidden = fixture.requests.length;
    await fixture.advance(page, 120_000);
    expect(fixture.requests).toHaveLength(hidden);
    await fixture.hidden(page, false);
    await fixture.recovered(page);
    await page.goto('about:blank');
    const closed = fixture.requests.length;
    await fixture.advance(page, 60_000);
    expect(fixture.requests).toHaveLength(closed);
    fixture.assertSafe();
  } finally {
    await fixture.finish(page, info);
  }
});

test('LOCAL capability refusals remain distinct and revoke retained files', async ({
  page,
  baseURL,
}, info) => {
  const fixture = new ComputerRecovery();
  const computer = fixture.computer(page);
  try {
    await fixture.route(page, baseURL!);
    await page.goto('/agents/agt_contract');
    await fixture.open(page);
    await fixture.recovered(page);
    await computer.getByRole('tab', { name: 'Files', exact: true }).click();
    await expect(computer.getByRole('textbox', { name: 'Contents of notes.txt' })).toHaveValue(
      fileText,
    );
    for (const [state, message] of [
      ['forbidden', 'You do not have access to these files.'],
      ['unsupported', 'This environment does not support file browsing.'],
      ['not_enabled', 'File browsing is not enabled for this environment.'],
      ['temporarily_unavailable', 'We couldn’t check file availability.'],
    ] as const) {
      fixture.phase = state;
      fixture.environment = {
        ...fixture.environment,
        files: { ...fixture.environment.files, state, writable: false },
      };
      await fixture.foreground(page);
      await expect(computer).toContainText(message);
      await expect(computer.getByRole('textbox', { name: 'Contents of notes.txt' })).toHaveCount(0);
      await expect(computer.getByRole('button', { name: 'New file', exact: true })).toHaveCount(0);
    }
    fixture.faults.set('environment', 403);
    await fixture.foreground(page);
    await expect(computer).toContainText('File access is unavailable.');
    await expect(computer).not.toContainText('We couldn’t check file availability.');
    fixture.faults.set('environment', 503);
    await fixture.foreground(page);
    await expect(computer).toContainText('We couldn’t check file availability.');
    fixture.faults.clear();
    fixture.environment = {
      ...fixture.environment,
      files: { ...fixture.environment.files, state: 'available', writable: true },
    };
    await fixture.advance(page, 30_000);
    await fixture.recovered(page);
    fixture.assertSafe();
  } finally {
    await fixture.finish(page, info);
  }
});

for (const transition of ['account', 'agent', 'revocation'] as const) {
  test(`LOCAL ${transition} change cancels old file reads and ignores their late success`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new ComputerRecovery();
    const computer = fixture.computer(page);
    try {
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      await fixture.open(page);
      await fixture.recovered(page);
      await computer.getByRole('tab', { name: 'Files', exact: true }).click();
      let releaseEnvironment = () => {};
      if (transition !== 'revocation') {
        const environmentBefore = fixture.count('environment');
        releaseEnvironment = fixture.hold('environment');
        await fixture.advance(page, 30_000);
        await expect.poll(() => fixture.count('environment')).toBe(environmentBefore + 1);
      }
      const before = fixture.count('files');
      const release = fixture.hold('files');
      await computer.getByRole('button', { name: 'notes.txt', exact: true }).click();
      await expect.poll(() => fixture.count('files')).toBe(before + 1);
      if (transition === 'account') {
        fixture.base.owner = 'other-owner';
        fixture.denied = true;
        await fixture.foreground(page);
      } else if (transition === 'agent') {
        await page.evaluate(() => {
          history.pushState({}, '', '/agents/agt_other');
          window.dispatchEvent(new PopStateEvent('popstate'));
        });
      } else {
        fixture.environment = {
          ...fixture.environment,
          files: { ...fixture.environment.files, state: 'forbidden', writable: false },
        };
        await fixture.foreground(page);
        await expect(computer).toContainText('You do not have access to these files.');
      }
      await expect
        .poll(() => fixture.cancelled.some((path) => path.endsWith('/files/content')))
        .toBe(true);
      const lateEnvironment =
        transition === 'revocation'
          ? undefined
          : page.waitForResponse((response) =>
              new URL(response.url()).pathname.endsWith('/environment'),
            );
      release();
      releaseEnvironment();
      if (lateEnvironment) {
        const response = await lateEnvironment;
        expect(response.status()).toBe(200);
        await response.finished();
      }
      await fixture.advance(page, 30_000);
      await expect(page.getByRole('textbox', { name: 'Contents of notes.txt' })).toHaveCount(0);
      await expect(page.getByRole('button', { name: 'New file', exact: true })).toHaveCount(0);
      if (transition !== 'revocation')
        await expect(page.getByText(operationText, { exact: true })).toHaveCount(0);
      fixture.assertSafe();
    } finally {
      await fixture.finish(page, info);
    }
  });
}
