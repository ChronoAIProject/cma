import { expect, test } from '@playwright/test';
import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { cursor, LocalAgentV2 } from './agent-v2.fixture';

// Minimal synthetic OOXML package with content types, a root relationship and a document.
const docx = Buffer.from(
  'UEsDBBQAAAAIAGKieV10JJxTuwAAAD4BAAATAAAAW0NvbnRlbnRfVHlwZXNdLnhtbJWQuQ7CMAyGX6XKiqgRAwNquwArMPACVuq2EbkUuxxvT8o1sDHa//FZrk73SFzcnPVcq0EkrgFYD+SQyxDJZ6ULyaHkMfUQUZ+xJ1guFivQwQt5mcvUoZpqSx2OVordLa/ZBF+rRJZVsXkZJ1atMEZrNErW4eLbH8r8TShz8unhwUSeZYOCpjpcKCXTUnHEJHt0uQ6uIbXQBj26jCgn41+80HVG0zc/tcUUNDEb3ztbfhWHxn/ugOfbmgdQSwMEFAAAAAgAYqJ5XWF7L0OJAAAA8gAAAAsAAABfcmVscy8ucmVsc43POw4CIRAG4KsQDrCzWlgYoLLZ1ngBAsMjLo8MGPX2UlisxsJy5p98f0accdU9ltxCrI090pqb5KH3egRoJmDSbSoV80hcoaT7GMlD1eaqPcJ+ng9AW4MrsTXZYiWnxe44uzwr/mMX56LBUzG3hLn/qPi6GLImj13yeyEL9r2eBstBCfh4Ub0AUEsDBBQAAAAIAGKieV0vI3/JegAAAKIAAAARAAAAd29yZC9kb2N1bWVudC54bWxFzsENwyAMBdBVogxQRz30gChLdAIKbkCKMTKuaLZvSA+9vC/b+pJtN5HDm7Do9KGtNNPvc1KtBqCFhOTbhSuW4/ZiIa/HKCt0lliFA7aWy0obXJflBuRzmZ3t5slxH1kHMlD32Ism1BwmwcqiFsZ2KKf19NeE/1fuC1BLAQIUAxQAAAAIAGKieV10JJxTuwAAAD4BAAATAAAAAAAAAAAAAACAAQAAAABbQ29udGVudF9UeXBlc10ueG1sUEsBAhQDFAAAAAgAYqJ5XWF7L0OJAAAA8gAAAAsAAAAAAAAAAAAAAIAB7AAAAF9yZWxzLy5yZWxzUEsBAhQDFAAAAAgAYqJ5XS8jf8l6AAAAogAAABEAAAAAAAAAAAAAAIABngEAAHdvcmQvZG9jdW1lbnQueG1sUEsFBgAAAAADAAMAuQAAAEcCAAAAAA==',
  'base64',
);

test('LOCAL keeps the selected file actions visible when an inline preview is unavailable', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: 'light' });
  const imageName = 'contact-sheet.jpg';
  const image = await page.evaluate(() => {
    const canvas = document.createElement('canvas');
    canvas.width = 1000;
    canvas.height = 800;
    const context = canvas.getContext('2d')!;
    for (let page = 0; page < 4; page++) {
      const x = 500 * (page % 2);
      const y = 400 * Math.floor(page / 2);
      context.fillStyle = 'white';
      context.fillRect(x + 16, y + 16, 468, 368);
      context.fillStyle = 'black';
      context.font = '24px serif';
      context.fillText(`Synthetic page ${page + 1}`, x + 32, y + 50);
    }
    return canvas.toDataURL('image/jpeg').split(',')[1]!;
  });
  const files = new Map([
    [imageName, Buffer.from(image, 'base64')],
    ['report.docx', docx],
    ['table.xlsx', Buffer.from([0x50, 0x4b, 3, 4, 0])],
    ['archive.zip', Buffer.from([0x50, 0x4b, 5, 6, 0])],
    ['print.pdf', Buffer.from('%PDF-1.7\n%\xe2\xe3\xcf\xd3\n', 'latin1')],
    ['unknown.bin', Buffer.from([0, 0xff, 1])],
    ['notes.txt', Buffer.from('Synthetic notes')],
  ]);
  let finishDocx!: () => void;
  const pendingDocx = new Promise<void>((resolve) => {
    finishDocx = resolve;
  });
  const downloads: string[] = [];
  try {
    await page.setViewportSize({ width: 1600, height: 1000 });
    await fixture.route(page, baseURL!);
    await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) => {
      const path = new URL(route.request().url()).searchParams.get('path')!;
      return route.fulfill({
        json: {
          path,
          entries: [...files].map(([name, bytes]) => ({
            path: path ? `${path}/${name}` : name,
            name,
            kind: 'file',
            size_bytes: bytes.length,
            hidden: false,
          })),
        },
      });
    });
    await page.route('**/api/v1/agents/agt_contract/files/content**', async (route) => {
      const path = new URL(route.request().url()).searchParams.get('path')!;
      const name = path.split('/').at(-1)!;
      if (name === 'report.docx') await pendingDocx;
      if (name === 'notes.txt')
        return route.fulfill({ status: 500, json: { code: 'store_failed' } });
      const bytes = files.get(name)!;
      await route.fulfill({
        json: {
          path,
          size_bytes: bytes.length,
          sha256: createHash('sha256').update(bytes).digest('hex'),
          content_base64: bytes.toString('base64'),
        },
      });
    });
    await page.route('**/api/v1/agents/agt_contract/files/download**', (route) => {
      const path = new URL(route.request().url()).searchParams.get('path')!;
      downloads.push(path);
      return route.fulfill({
        body: files.get(path.split('/').at(-1)!)!,
        contentType: 'application/octet-stream',
      });
    });
    await page.goto('/agents/agt_contract');
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    await computer.getByRole('tab', { name: 'Files', exact: true }).click();
    const preview = computer.getByRole('region', { name: 'File preview' });
    const fileRow = (name: string) => computer.getByRole('button', { name, exact: true });
    const download = preview.getByRole('button', { name: 'Download file' });
    await expect(preview).toBeHidden();
    await fileRow(imageName).click();
    const viewport = preview.getByRole('region', { name: 'Image viewport' });
    const picture = viewport.getByRole('img', { name: imageName });
    const box = (await viewport.boundingBox())!;
    await preview.getByRole('button', { name: '100%', exact: true }).click();
    await expect(picture).toHaveCSS('width', '1000px');
    for (let step = 0; step < 7; step++) await viewport.press('+');
    await expect(picture).toHaveCSS('width', '4000px');
    expect(await viewport.boundingBox()).toEqual(box);
    await page.mouse.move(box.x + 100, box.y + 100);
    await page.mouse.down();
    await page.mouse.move(box.x + 35, box.y + 40);
    await page.mouse.up();
    await expect.poll(() => viewport.evaluate((node) => node.scrollLeft)).toBeGreaterThan(0);
    await viewport.press('-');
    await expect(picture).toHaveCSS('width', '3200px');
    for (const modifier of ['Control', 'Meta']) {
      await page.keyboard.down(modifier);
      await page.mouse.wheel(0, -120);
      await page.keyboard.up(modifier);
      await expect(picture).toHaveCSS('width', '4000px');
      await viewport.press('-');
    }
    await viewport.press('0');
    await expect(picture).toBeInViewport({ ratio: 1 });
    await preview.getByRole('button', { name: 'Full screen' }).click();
    const dialog = page.getByRole('dialog', { name: imageName });
    await page.setViewportSize({ width: 390, height: 700 });
    await page.emulateMedia({ colorScheme: 'dark' });
    const fullViewport = dialog.getByRole('region', { name: 'Image viewport' });
    await expect(fullViewport.getByRole('img', { name: imageName })).toBeInViewport({ ratio: 1 });
    expect((await fullViewport.boundingBox())!.height).toBeGreaterThan(500);
    await dialog.getByRole('button', { name: 'Zoom in' }).click();
    await dialog.getByRole('button', { name: 'Zoom out' }).click();
    await dialog.getByRole('button', { name: 'Fit' }).click();
    await expect(fullViewport.getByRole('img', { name: imageName })).toBeInViewport({ ratio: 1 });
    await fullViewport.press('Escape');
    await expect(dialog).toBeHidden();
    await expect(preview.getByRole('button', { name: 'Full screen' })).toBeFocused();
    await page.setViewportSize({ width: 1600, height: 1000 });
    await expect(download).toBeEnabled();

    await fileRow('report.docx').click();
    await expect(preview.getByText('Opening file…')).toBeVisible();
    await expect(download).toBeEnabled();
    finishDocx();
    await expect(preview.getByText('Opening file…')).toBeHidden();
    for (const name of ['report.docx', 'table.xlsx', 'archive.zip', 'print.pdf', 'unknown.bin']) {
      if (name !== 'report.docx') await fileRow(name).click();
      await expect(fileRow(name)).toHaveAttribute('aria-current', 'true');
      await expect(
        preview.getByLabel('Selected file').filter({ hasText: new RegExp(`/home/.*${name}$`) }),
      ).toBeVisible();
      await expect(
        preview.getByText('Preview unavailable. Download this file to view its contents.'),
      ).toBeVisible();
      await expect(preview.getByRole('button', { name: 'Delete file' })).toBeEnabled();
      await expect(download).toBeEnabled();
      if (name === 'report.docx') {
        expect(downloads).toEqual([]);
        const saving = page.waitForEvent('download');
        await download.click();
        const saved = await saving;
        expect(saved.suggestedFilename()).toBe(name);
        expect(await readFile((await saved.path())!)).toEqual(docx);
      }
    }
    await fileRow('notes.txt').click();
    await expect(preview.getByRole('button', { name: 'Retry preview' })).toBeVisible();
    await expect(download).toBeEnabled();
    expect(downloads).toHaveLength(1);
    expect(downloads[0]).toMatch(/(?:^|\/)report\.docx$/);
    expect(fixture.writes).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
  } finally {
    finishDocx();
    await fixture.close(page);
  }
});

for (const stalled of ['headers', 'body'])
  test(`LOCAL stalled ${stalled} preview preserves input and offers a bounded retry`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    let reads = 0;
    let cancelled = 0;
    const server = createServer((request, response) => {
      response.setHeader('Access-Control-Allow-Origin', baseURL!);
      response.setHeader('Access-Control-Allow-Credentials', 'true');
      response.setHeader('Content-Type', 'application/json');
      reads += 1;
      if (reads === 1) {
        response.on('close', () => {
          cancelled += 1;
        });
        if (stalled === 'body') response.write('{"path":');
        return;
      }
      const path = new URL(request.url!, baseURL).searchParams.get('path');
      response.end(
        JSON.stringify({
          path,
          size_bytes: 5,
          sha256: 'a'.repeat(64),
          content_base64: Buffer.from('Notes').toString('base64'),
        }),
      );
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    if (!address || typeof address === 'string') throw new Error('No file fixture port');
    const origin = `http://127.0.0.1:${address.port}`;
    try {
      await page.setViewportSize({ width: 1600, height: 1000 });
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
      await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) => {
        const path = new URL(route.request().url()).searchParams.get('path')!;
        return route.fulfill({
          json: {
            path,
            entries: [
              {
                path: `${path ? `${path}/` : ''}notes.txt`,
                name: 'notes.txt',
                kind: 'file',
                size_bytes: 5,
                hidden: false,
              },
            ],
          },
        });
      });
      await page.route(`${origin}/**`, (route) => route.continue());
      await page.route('**/api/v1/agents/agt_contract/files/content**', (route) => {
        const url = new URL(route.request().url());
        return route.continue({ url: `${origin}${url.pathname}${url.search}` });
      });
      await page.goto('/agents/agt_contract');
      await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
      const message = page.getByRole('textbox', { name: 'Message', exact: true });
      await expect(message).toBeEnabled();
      const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
      await computer.getByRole('tab', { name: 'Files', exact: true }).click();
      await computer.getByRole('button', { name: 'notes.txt', exact: true }).click();
      await expect(computer.getByText('Opening file…')).toBeVisible();
      await message.fill('My draft remains usable during a stalled file read.');
      await expect(computer.getByRole('button', { name: 'Retry preview' })).toBeVisible({
        timeout: 6000,
      });
      expect(reads).toBe(1);
      await expect.poll(() => cancelled).toBe(1);
      await computer.getByRole('button', { name: 'Retry preview' }).click();
      await expect(computer.getByRole('textbox', { name: 'Contents of notes.txt' })).toHaveValue(
        'Notes',
      );
      await expect(message).toHaveValue('My draft remains usable during a stalled file read.');
      expect(reads).toBe(2);
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      await page.screenshot({ path: info.outputPath('preview-after-retry.png') });
    } catch (error) {
      await info.attach('preview-dom.txt', {
        body: await page.locator('body').innerText(),
        contentType: 'text/plain',
      });
      throw error;
    } finally {
      await fixture.close(page);
      server.closeAllConnections();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
