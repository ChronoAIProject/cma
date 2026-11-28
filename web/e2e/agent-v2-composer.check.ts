import { test, expect } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';

for (const theme of ['light', 'dark']) {
  test(`LOCAL ${theme}: queue ordinary input during an active response without steering it`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.queued.enabled = true;
    fixture.append(
      { type: 'turn_started', turn_sequence: 1 },
      {
        response_id: 'resp_active',
        command_id: 'cmd_active',
        turn_sequence: 1,
      },
    );
    try {
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      const field = page.getByRole('textbox', { name: 'Message', exact: true });
      const queue = page.getByRole('button', { name: 'Queue message', exact: true });
      const stop = page.getByRole('button', { name: 'Stop response', exact: true });
      const stopScope = page.getByText('Running Sandbox commands may continue.', { exact: true });
      await expect(queue).toHaveCount(0);
      await expect(stop).toBeEnabled();
      await field.fill('Continue with this after the active response');
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 });
        await field.fill('');
        await expect(stop).toBeEnabled();
        // Finish the frame's resize measurement and column transition before
        // comparing the controls at this viewport width.
        await page.locator('.frame').evaluate(async (frame) => {
          await document.fonts.ready;
          await new Promise<void>((resolve) => {
            requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
          });
          await Promise.all(frame.getAnimations().map((animation) => animation.finished));
        });
        const stopBox = await stop.boundingBox();
        await expect(stopScope).toBeInViewport();
        const scopeBox = await stopScope.boundingBox();
        expect(scopeBox!.width).toBeGreaterThan(1);
        expect(scopeBox!.x).toBeGreaterThanOrEqual(0);
        expect(scopeBox!.x + scopeBox!.width).toBeLessThanOrEqual(width);
        expect(scopeBox!.y + scopeBox!.height).toBeLessThanOrEqual(stopBox!.y);
        await field.fill('Continue with this after the active response');
        await expect(queue).toBeEnabled();
        expect(await queue.boundingBox()).toEqual(stopBox);
        await expect(stop).toHaveCount(0);
        await expect(stopScope).toHaveCount(0);
        await expect
          .poll(() =>
            queue.evaluate((element) => {
              const box = element.getBoundingClientRect();
              return (
                box.left >= 0 &&
                box.right <= innerWidth &&
                box.top >= 0 &&
                box.bottom <= innerHeight
              );
            }),
          )
          .toBe(true);
        await page.screenshot({
          path: info.outputPath(`queued-composer-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      await expect(queue).toBeEnabled();
      await field.press('Enter');
      await expect(field).toHaveValue('');
      await expect(
        page.getByRole('button', { name: 'Remove queued message 1', exact: true }),
      ).toBeEnabled();
      expect(fixture.writes).toHaveLength(1);
      expect(fixture.writes[0]!.path).toBe('/api/v2/agents/agt_contract/responses');
      expect(fixture.writes[0]!.body).toEqual({
        input: 'Continue with this after the active response',
        stream: true,
      });
      await expect(
        page.getByRole('status').filter({ hasText: /^Waiting for a response$/ }),
      ).toBeVisible();
      await expect(stop).toBeEnabled();
      fixture.append({
        type: 'tool_started',
        call_id: 'ongoing',
        title: 'Continue original work',
        kind: 'execute',
        status: 'running',
      });
      await page.getByRole('button', { name: 'View activity', exact: true }).click();
      await expect(page.locator('.thinking__activity')).toContainText('Continue original work');
      await field.fill('Keep this next draft');
      await page.getByRole('button', { name: 'Remove queued message 1', exact: true }).click();
      await expect(
        page.getByRole('button', { name: 'Remove queued message 1', exact: true }),
      ).toHaveCount(0);
      await expect(field).toHaveValue('Keep this next draft');
      await expect(queue).toBeEnabled();
      await expect(stop).toHaveCount(0);
      expect(fixture.writes.filter((write) => write.path.endsWith('/controls'))).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}

for (const theme of ['light', 'dark']) {
  test(`LOCAL ${theme}: draft grows upward inside the box without moving its controls or covering history`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    try {
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      const field = page.getByRole('textbox', { name: 'Message', exact: true });
      const send = page.getByRole('button', { name: 'Send message', exact: true });
      await expect(field).toBeEnabled();
      await expect(page.getByRole('button', { name: 'Attach files' })).toBeEnabled();
      await page.evaluate(() => document.fonts.ready.then(() => undefined));
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 });
        await field.fill('');
        // The frame measures its columns in ResizeObserver; finish that layout
        // before comparing expansion against the compact seat at this width.
        await page.evaluate(
          () =>
            new Promise<void>((resolve) => {
              requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
            }),
        );
        const initial = await field.boundingBox();
        const control = await send.boundingBox();
        const seat = await page.locator('.agent-composer__form').boundingBox();
        expect(initial?.height).toBe(40);
        expect(control?.height).toBe(width > 480 ? 32 : 44);
        const padding = await field.evaluate((element) => {
          const style = getComputedStyle(element);
          return [style.paddingTop, style.paddingBottom];
        });
        expect(padding[0]).toBe(padding[1]);
        const text = Array.from(
          { length: 25 },
          (_, index) => `Line ${index + 1}: keep my draft.`,
        ).join('\n');
        await field.fill(text);
        const expanded = await field.boundingBox();
        expect(expanded!.height).toBeGreaterThan(initial!.height);
        expect(expanded!.height).toBeLessThanOrEqual(336);
        expect(expanded!.y).toBeLessThan(initial!.y);
        expect(expanded!.y + expanded!.height).toBe(initial!.y + initial!.height);
        expect(await send.boundingBox()).toEqual(control);
        // The box grows upward from a stable bottom edge and pushes the
        // conversation up rather than covering it.
        const grown = await page.locator('.agent-composer__form').boundingBox();
        expect(grown!.y + grown!.height).toBe(seat!.y + seat!.height);
        expect(grown!.y).toBeLessThan(seat!.y);
        const conversation = await page.locator('.agent-surface__conversation').boundingBox();
        expect(conversation!.y + conversation!.height).toBeLessThanOrEqual(grown!.y);
        await field.evaluate((element: HTMLTextAreaElement) => {
          element.setSelectionRange(8, 12);
          element.blur();
        });
        await page.mouse.move(0, 0);
        // A multi-line draft keeps its height when the box loses focus.
        expect((await field.boundingBox())?.height).toBe(expanded!.height);
        expect(
          await field.evaluate((element: HTMLTextAreaElement) => [
            element.selectionStart,
            element.selectionEnd,
          ]),
        ).toEqual([8, 12]);
        await expect(field).toHaveValue(text);
        expect(await send.boundingBox()).toEqual(control);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        await page.screenshot({
          path: info.outputPath(`compact-draft-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}

test('LOCAL browser clipboard screenshot and dropped file preview, persist, and send together to the same Agent', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  fixture.holdInput = true;
  const uploads: {
    path: string;
    name: string;
    size: number;
    key: string;
    type: string;
    bytes: Buffer;
  }[] = [];
  let releaseImage: (() => void) | undefined;
  const imageReady = new Promise<void>((resolve) => {
    releaseImage = resolve;
  });
  let failNotes = true;
  try {
    await fixture.route(page, baseURL!);
    await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
    await page.route('**/api/v1/agents/agt_contract/attachments?*', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      const name = url.searchParams.get('name')!;
      const bytes = request.postDataBuffer()!;
      uploads.push({
        path: url.pathname,
        name,
        size: bytes.byteLength,
        bytes,
        key: request.headers()['idempotency-key']!,
        type: request.headers()['content-type']!,
      });
      if (name === 'image.png') await imageReady;
      if (name === 'notes.txt' && failNotes) {
        failNotes = false;
        return route.fulfill({
          status: 503,
          json: {
            code: 'runtime_unavailable',
            detail: 'Local upload interrupted. Retry the file.',
          },
        });
      }
      const id = `att_${(name === 'image.png' ? 'a' : 'b').repeat(64)}`;
      return route.fulfill({
        status: 201,
        json: {
          id,
          agent_id: 'agt_contract',
          name,
          path: `/home/attachments/agt_contract/${id}/${name}`,
          size_bytes: bytes.byteLength,
          media_type: request.headers()['content-type'],
          sha256: 'c'.repeat(64),
        },
      });
    });
    await page.goto('/agents/agt_contract');
    const field = page.getByRole('textbox', { name: 'Message', exact: true });
    const send = page.getByRole('button', { name: 'Send message', exact: true });
    await expect(page.getByRole('button', { name: 'Attach files' })).toBeEnabled();
    await field.fill('Read the screenshot and the notes together.');
    // Use the browser clipboard and keyboard paste, not a mocked paste event.
    await page.evaluate(async () => {
      const canvas = document.createElement('canvas');
      canvas.width = 2;
      canvas.height = 2;
      const context = canvas.getContext('2d')!;
      context.fillStyle = '#008000';
      context.fillRect(0, 0, 2, 2);
      const image = await new Promise<Blob>((resolve) =>
        canvas.toBlob((blob) => resolve(blob!), 'image/png'),
      );
      await navigator.clipboard.write([new ClipboardItem({ 'image/png': image })]);
    });
    await field.press(process.platform === 'darwin' ? 'Meta+V' : 'Control+V');
    await expect(page.getByRole('list', { name: 'Attachments' })).toContainText('image.png');
    await expect(page.locator('.composer-attachments__preview img')).toHaveJSProperty(
      'naturalWidth',
      2,
    );
    await expect(send).toBeDisabled();
    expect(fixture.writes).toEqual([]);
    releaseImage!();
    await expect(page.getByRole('list', { name: 'Attachments' })).toContainText('Ready');
    const transfer = await page.evaluateHandle(() => {
      const transfer = new DataTransfer();
      transfer.items.add(new File(['Local attachment note.'], 'notes.txt', { type: 'text/plain' }));
      return transfer;
    });
    await field.dispatchEvent('drop', { dataTransfer: transfer });
    await transfer.dispose();
    await expect(page.getByRole('list', { name: 'Attachments' })).toContainText(
      'Local upload interrupted',
    );
    await expect(send).toBeDisabled();
    await page.getByRole('button', { name: 'Retry', exact: true }).click();
    await expect(send).toBeEnabled();
    expect(uploads).toHaveLength(3);
    expect(uploads[0]!.bytes.subarray(0, 8).toString('hex')).toBe('89504e470d0a1a0a');
    expect(uploads[1]!.bytes.toString()).toBe('Local attachment note.');
    expect(uploads[2]!.key).toBe(uploads[1]!.key);
    expect(
      uploads.every((upload) => upload.path === '/api/v1/agents/agt_contract/attachments'),
    ).toBe(true);
    await send.click();
    await expect.poll(() => fixture.writes.length).toBe(1);
    await expect(field).toHaveValue('');
    await expect(page.getByRole('list', { name: 'Attachments' })).toHaveCount(0);
    const outgoing = page.locator('.message--user').filter({ hasText: 'Sending…' });
    await expect(outgoing).toContainText('Read the screenshot and the notes together.');
    await expect(outgoing).toContainText('image.png');
    await expect(outgoing).toContainText('notes.txt');
    expect(fixture.writes[0]!.path).toBe('/api/v2/agents/agt_contract/responses');
    const input = fixture.writes[0]!.body.input as string;
    expect(input).toContain('Read the screenshot and the notes together.');
    expect(input).toContain(`/home/attachments/agt_contract/att_${'a'.repeat(64)}/image.png`);
    expect(input).toContain(`/home/attachments/agt_contract/att_${'b'.repeat(64)}/notes.txt`);
    await field.fill('Keep my newer draft while the previous files are accepted.');
    fixture.acceptInput();
    await expect(page.locator('.message__delivery')).toHaveText('Sent');
    await expect(field).toHaveValue('Keep my newer draft while the previous files are accepted.');
    await expect(page.getByRole('list', { name: 'Attachments' })).toHaveCount(0);
    expect(fixture.boundary.writes).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
    expect(fixture.boundary.errors).toEqual([]);
    expect(fixture.errors).toEqual([]);
  } finally {
    releaseImage?.();
    await fixture.close(page);
  }
});

for (const action of ['button', 'Escape'] as const) {
  test(`LOCAL response Stop: ${action} submits one fenced cancellation and retains its outcome`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    fixture.append(
      { type: 'turn_started', turn_sequence: 1 },
      {
        response_id: 'resp_1',
        command_id: 'cmd_1',
        turn_sequence: 1,
      },
    );
    let release!: () => void;
    const pending = new Promise<void>((resolve) => {
      release = resolve;
    });
    let attempts = 0;
    try {
      await fixture.route(page, baseURL!);
      await page.route('**/responses/resp_1/controls', async (route) => {
        if (route.request().method() === 'POST') {
          attempts += 1;
          await pending;
        }
        await route.fallback();
      });
      await page.goto('/agents/agt_contract');
      const field = page.getByRole('textbox', { name: 'Message', exact: true });
      const stop = page.getByRole('button', { name: 'Stop response', exact: true });
      await expect(stop).toBeEnabled();
      await field.focus();
      await expect(page.getByText('Esc to stop', { exact: true })).toBeVisible();
      await expect(stop).toHaveAccessibleDescription(
        'Esc to stop Running Sandbox commands may continue.',
      );
      if (action === 'Escape') await field.press('Escape');
      else await stop.click();
      await expect.poll(() => attempts).toBe(1);
      await expect(page.getByRole('button', { name: 'Stopping response' })).toBeDisabled();
      await field.press('Escape');
      await field.press('Escape');
      expect(attempts).toBe(1);
      await expect(page.getByText('Esc to stop', { exact: true })).toHaveCount(0);
      release();
      await expect.poll(() => fixture.writes.length).toBe(1);
      expect(fixture.writes[0]).toMatchObject({
        path: '/api/v2/agents/agt_contract/responses/resp_1/controls',
        body: { turn_sequence: 1, expected_agent_revision: 10, command: { type: 'cancel' } },
      });
      expect(fixture.writes[0]!.key).toBeTruthy();
      const terminal = page.getByRole('status').filter({
        hasText: /Response interrupted.*Running Sandbox commands may continue/,
      });
      await expect(terminal).toHaveCount(0);
      fixture.append({ type: 'turn_cancelled', turn_sequence: 1 });
      await expect(terminal).toBeVisible();
      await expect(page.getByRole('button', { name: /Stop.*response/ })).toHaveCount(0);
      await page.reload();
      await expect(field).toBeVisible();
      await field.press('Escape');
      expect(fixture.writes).toHaveLength(1);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      release();
      await fixture.close(page);
    }
  });
}

test('LOCAL unbound response never advertises Stop; focus outside the composer cannot cancel it', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  fixture.append({ type: 'turn_started', turn_sequence: 1 });
  try {
    await fixture.route(page, baseURL!);
    await page.goto('/agents/agt_contract');
    const field = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(page.getByRole('button', { name: 'Stop response', exact: true })).toBeDisabled();
    await expect(
      page.getByText('Stop is unavailable because this turn has no bound response.', {
        exact: true,
      }),
    ).toBeVisible();
    await field.press('Escape');
    await expect(page.getByText('Esc to stop', { exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Show Agent details', exact: true }).focus();
    await page.keyboard.press('Escape');
    expect(fixture.writes).toEqual([]);
    await page.reload();
    await expect(page.getByRole('button', { name: 'Stop response', exact: true })).toBeDisabled();
    await expect(page.getByText('Esc to stop', { exact: true })).toHaveCount(0);
    expect(fixture.writes).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
    expect(fixture.errors).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});
