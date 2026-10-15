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
      await expect(queue).toBeVisible();
      await expect(stop).toBeEnabled();
      await field.fill('Continue with this after the active response');
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 });
        for (const button of [queue, stop]) {
          await expect
            .poll(() =>
              button.evaluate((element) => {
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
        }
        await expect
          .poll(async () => {
            const [queueBox, stopBox] = await Promise.all([
              queue.boundingBox(),
              stop.boundingBox(),
            ]);
            if (queueBox === null || stopBox === null) return Number.POSITIVE_INFINITY;
            return Math.abs(queueBox.y + queueBox.height / 2 - (stopBox.y + stopBox.height / 2));
          })
          .toBeLessThan(1);
        await page.screenshot({
          path: info.outputPath(`queued-composer-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      await field.press('Enter');
      await expect(field).toHaveValue('');
      await expect(
        page.getByRole('button', { name: 'Cancel queued message', exact: true }),
      ).toBeEnabled();
      expect(fixture.writes).toHaveLength(1);
      expect(fixture.writes[0]!.path).toBe('/api/v2/agents/agt_contract/responses');
      expect(fixture.writes[0]!.body).toEqual({
        input: 'Continue with this after the active response',
        stream: true,
      });
      await expect(page.getByRole('button', { name: /^Thinking/ })).toBeVisible();
      await expect(stop).toBeEnabled();
      fixture.append({
        type: 'tool_started',
        call_id: 'ongoing',
        title: 'Continue original work',
        kind: 'execute',
        status: 'running',
      });
      await expect(page.getByRole('button', { name: /^Thinking/ })).toContainText(
        'Continue original work',
      );
      await field.fill('Keep this next draft');
      await page.getByRole('button', { name: 'Cancel queued message', exact: true }).click();
      await expect(
        page
          .getByRole('status')
          .filter({ hasText: 'Your queued message was cancelled before it started.' }),
      ).toBeVisible();
      await expect(field).toHaveValue('Keep this next draft');
      await expect(stop).toBeEnabled();
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
  test(`LOCAL ${theme}: compact draft expands upward without moving its controls or history`, async ({
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
        const history = await page.locator('.message-stream').boundingBox();
        expect(initial?.height).toBe(width > 480 ? 36 : 44);
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
        expect(await page.locator('.agent-composer__form').boundingBox()).toEqual(seat);
        expect(await page.locator('.message-stream').boundingBox()).toEqual(history);
        await field.evaluate((element: HTMLTextAreaElement) => {
          element.setSelectionRange(8, 12);
          element.blur();
        });
        await page.mouse.move(0, 0);
        await expect.poll(async () => (await field.boundingBox())?.height).toBe(initial!.height);
        await field.hover();
        await expect.poll(async () => (await field.boundingBox())?.height).toBe(expanded!.height);
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
    await expect(field).toHaveValue('Read the screenshot and the notes together.');
    await expect(page.getByRole('list', { name: 'Attachments' })).toBeVisible();
    expect(fixture.writes[0]!.path).toBe('/api/v2/agents/agt_contract/responses');
    const input = fixture.writes[0]!.body.input as string;
    expect(input).toContain('Read the screenshot and the notes together.');
    expect(input).toContain(`/home/attachments/agt_contract/att_${'a'.repeat(64)}/image.png`);
    expect(input).toContain(`/home/attachments/agt_contract/att_${'b'.repeat(64)}/notes.txt`);
    fixture.acceptInput();
    await expect(field).toHaveValue('');
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
