import { test, expect, type Page, type TestInfo } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('LOCAL long tool paths preserve visible status at desktop and mobile widths', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  const title = `Read file '/home/cma/research/.worktrees/${'worktree-'.repeat(12)}/CLAUDE.md'`;
  boundary.events = [
    {
      seq: 1,
      appended_at: localTime,
      kind: {
        type: 'tool_call_started',
        call_id: 'local-read',
        title,
        kind: 'read',
        status: 'completed',
      },
    },
  ];
  await boundary.route(page, baseURL!);
  await page.goto('/agents/agt_contract');
  await expect(page.locator('.agent-event--tool strong')).toHaveText(title);
  for (const width of [1440, 390, 320]) {
    await page.setViewportSize({ width, height: 900 });
    await expect
      .poll(() =>
        page.locator('.agent-event--tool').evaluate((row) => {
          const bounds = row.getBoundingClientRect();
          return {
            rowFits: bounds.left >= 0 && bounds.right <= innerWidth + 1,
            overflowing: [...row.querySelectorAll<HTMLElement>('strong, span')].filter(
              (child) =>
                child.scrollWidth > child.clientWidth + 1 ||
                child.getBoundingClientRect().right > bounds.right ||
                child.getBoundingClientRect().left < bounds.left,
            ).length,
          };
        }),
      )
      .toEqual({ rowFits: true, overflowing: 0 });
    await page.screenshot({
      path: info.outputPath(`tool-path-${width}.png`),
      fullPage: true,
      animations: 'disabled',
    });
  }
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});

async function captureLayouts(page: Page, info: TestInfo, state: string) {
  const originalViewport = page.viewportSize();
  const originallyDark = await page.evaluate(() =>
    document.body.hasAttribute('data-ds-dark-theme'),
  );
  try {
    for (const theme of ['light', 'dark']) {
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const viewport of [
        { width: 1440, height: 1000 },
        { width: 390, height: 844 },
        { width: 320, height: 740 },
      ]) {
        await page.setViewportSize(viewport);
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        ).toBe(true);
        await expect
          .poll(() =>
            page.evaluate(() =>
              [
                ...document.querySelectorAll<HTMLElement>(
                  '.agent-sign-in, .message-stream__recovery, [role="dialog"]',
                ),
              ]
                .filter(
                  (element) =>
                    element.clientWidth > 0 &&
                    (element.scrollWidth > element.clientWidth + 1 ||
                      element.getBoundingClientRect().left < -1 ||
                      element.getBoundingClientRect().right > innerWidth + 1),
                )
                .map((element) => ({
                  className: element.className,
                  width: element.clientWidth,
                  contentWidth: element.scrollWidth,
                  left: element.getBoundingClientRect().left,
                  right: element.getBoundingClientRect().right,
                  viewport: innerWidth,
                })),
            ),
          )
          .toEqual([]);
        const signInLink = await page.evaluate(() => {
          const link = document.querySelector('.agent-sign-in > a');
          if (link === null) return undefined;
          const probe = document.createElement('span');
          probe.style.color = 'var(--dsw-alias-state-business-primary)';
          document.body.append(probe);
          const expected = getComputedStyle(probe).color;
          probe.remove();
          return { actual: getComputedStyle(link).color, expected };
        });
        if (signInLink) expect(signInLink.actual).toBe(signInLink.expected);
        await page.screenshot({
          path: info.outputPath(`${state}-${theme}-${viewport.width}.png`),
          fullPage: true,
        });
      }
    }
  } finally {
    await page.evaluate(
      (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
      originallyDark,
    );
    if (originalViewport) await page.setViewportSize(originalViewport);
  }
}

for (const status of ['paused', 'reviving'] as const) {
  for (const kind of ['workspace', 'agent'] as const) {
    test(`LOCAL ${status} parent blocks ${kind} creation without a browser write`, async ({
      page,
      baseURL,
    }) => {
      const boundary = new LocalStartup();
      boundary.sandbox = { ...boundary.sandbox, status };
      await boundary.route(page, baseURL!);
      await page.goto('/sandboxes/sbx_route');
      const surface = page.locator('.sandbox-page');
      await expect(surface.getByRole('heading', { name: 'research', exact: true })).toBeVisible();
      if (kind === 'workspace')
        await expect(
          surface.getByRole('button', { name: 'New workspace', exact: true }),
        ).toBeDisabled();
      else {
        await expect(surface.getByRole('button', { name: 'New agent', exact: true })).toHaveCount(
          0,
        );
        await expect(
          surface.getByRole('button', { name: 'New agent in this Sandbox', exact: true }),
        ).toBeDisabled();
      }
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }
}

for (const kind of ['workspace'] as const) {
  test(`LOCAL active ${kind} creation retries one command and reloads without manual revival`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    boundary.loseCreationResponse = true;
    await boundary.route(page, baseURL!);
    await page.goto('/sandboxes/sbx_route');
    await page
      .locator('.sandbox-page')
      .getByRole('button', { name: `New ${kind}`, exact: true })
      .click();
    const dialog = page.getByRole('dialog', { name: `New ${kind}`, exact: true });
    await dialog.getByRole('button', { name: `Create ${kind}`, exact: true }).click();
    await expect(dialog.getByRole('button', { name: 'Retry creation', exact: true })).toBeEnabled();
    expect(boundary.writes).toHaveLength(1);
    await captureLayouts(page, info, 'unsettled-creation');
    await dialog.getByRole('button', { name: 'Retry creation', exact: true }).click();
    await expect(dialog).toHaveCount(0);
    expect(boundary.writes).toHaveLength(2);
    expect(boundary.writes[1]).toEqual(boundary.writes[0]);
    expect(boundary.writes[0]?.key).toBeTruthy();
    expect(boundary.writes[0]?.body).toEqual({ source: { type: 'empty' }, name: 'workspace' });
    await page.reload();
    await expect(
      page.getByRole('heading', {
        name: 'research',
        exact: true,
      }),
    ).toBeVisible();
    await captureLayouts(page, info, 'accepted-reload');
    expect(boundary.writes).toHaveLength(2);
    expect(boundary.writes.some((write) => write.path.endsWith('/revive'))).toBe(false);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
    await info.attach('local-boundary', {
      body: JSON.stringify({
        mocked: true,
        boundary: 'built_browser_public_http',
        status: 'active',
        kind,
        writes: 2,
        commands: 1,
        captures: 12,
      }),
      contentType: 'application/json',
    });
  });
}

test('LOCAL provider sign-in replays after reload and clears durably without a fake turn', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  boundary.agent = { ...boundary.agent, status: 'starting' };
  boundary.events = [
    {
      seq: 1,
      appended_at: localTime,
      kind: {
        type: 'sign_in_required',
        message: 'Authorize the local provider fixture.',
        code: 'LOCAL-CODE',
        url: 'https://accounts.example.test/device',
      },
    },
    {
      seq: 2,
      appended_at: localTime,
      kind: { type: 'first_message_changed', state: 'recovery_required', recovery_generation: 1 },
    },
  ];
  await boundary.route(page, baseURL!);
  await page.goto('/agents/agt_contract');
  await expect(page.getByRole('region', { name: 'Sign in to your account' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Continue sign in' })).toHaveAttribute(
    'href',
    'https://accounts.example.test/device',
  );
  await page.reload();
  await expect(page.getByText('LOCAL-CODE')).toBeVisible();
  await expect(page.getByText('First message needs recovery', { exact: true })).toBeVisible();
  await captureLayouts(page, info, 'sign-in-replayed');
  boundary.events.push({
    seq: 3,
    appended_at: localTime,
    kind: {
      type: 'conversation_opened',
      conversation_id: 'local-conversation',
      runtime_generation: 1,
    },
  });
  boundary.agent = { ...boundary.agent, status: 'ready' };
  await expect(page.getByRole('region', { name: 'Sign in to your account' })).toHaveCount(0);
  await expect(page.getByText('First message needs recovery', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop response', exact: true })).toHaveCount(0);
  await page.reload();
  await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeVisible();
  await expect(page.getByText('LOCAL-CODE')).toHaveCount(0);
  await expect(page.getByText('First message needs recovery', { exact: true })).toBeVisible();
  await captureLayouts(page, info, 'sign-in-cleared');
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
  await info.attach('local-boundary', {
    body: JSON.stringify({
      mocked: true,
      boundary: 'durable_public_event_replay',
      turnsStarted: 0,
      providerLoginExecuted: false,
      captures: 12,
    }),
    contentType: 'application/json',
  });
});
