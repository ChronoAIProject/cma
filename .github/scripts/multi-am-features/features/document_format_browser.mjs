import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {check, hash, id, other, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const timeout = 15000;
const pixel = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jPZkAAAAASUVORK5CYII=', 'base64');
const markers = ['Fixture heading', 'Fixture paragraph', 'Fixture font', 'Fixture bold', 'Fixture italic',
  'Fixture underline', 'Fixture bullets', 'Fixture numbers', 'Fixture checklist', 'Fixture quote',
  'Fixture code', 'Mermaid anchor', 'Details anchor', 'Date anchor', 'Divider anchor',
  'Image anchor', 'Table anchor', 'Temporary table anchor'];

async function verifyRendered(content, image) {
  for (const [selector, marker] of [
    ['h2', 'Fixture heading'], ['p', 'Fixture paragraph'], ['span[data-font="mono"]', 'Fixture font'],
    ['strong', 'Fixture bold'], ['em', 'Fixture italic'], ['u', 'Fixture underline'],
    ['ul:not([data-type="taskList"]) li', 'Fixture bullets'], ['ol li', 'Fixture numbers'],
    ['[data-type="taskItem"]', 'Fixture checklist'], ['blockquote', 'Fixture quote'],
    ['pre', 'Fixture code'], ['[data-type="details"] summary', 'Details'],
  ]) await content.locator(selector).filter({hasText: marker}).first().waitFor({timeout});
  await content.locator('[data-type="details"]').getByText('Add details here.', {exact: true}).waitFor({timeout});
  await content.locator('time[datetime]').waitFor({timeout});
  check(/^\d{4}-\d{2}-\d{2}$/.test(await content.locator('time').getAttribute('datetime')),
    'document_format_date_unbound');
  await content.locator('hr').waitFor({timeout});
  const picture = content.getByRole('img', {name: 'Fixture pixel', exact: true});
  await picture.scrollIntoViewIfNeeded({timeout});
  check(await picture.getAttribute('src') === image
    && await picture.getAttribute('referrerpolicy') === 'no-referrer', 'document_format_image_unbound');
  await poll(() => picture.evaluate(element => element.complete && element.naturalWidth === 1),
    Boolean, {timeout, interval: 50, code: 'document_format_image_not_rendered'});
  const table = content.getByRole('table');
  check(await table.count() === 1 && await table.getByRole('row').count() === 4
    && await table.getByRole('row').first().getByRole('columnheader').count() === 4,
  'document_format_table_dimensions_changed');
  await table.getByText('Fixture cell', {exact: true}).waitFor({timeout});
}

export async function documentFormat({page, ingress, user, documentId, recordFailure}) {
  check(id(documentId), 'document_format_identity_required');
  const path = '/api/v1/documents/' + documentId;
  const peer = other(ingress);
  const read = async (am, suffix = '') => (await user.request(am, 'GET', path + suffix)).body;
  const original = await read(peer);
  const originalSource = await read(peer, '/source');
  check(original.id === documentId && original.access === 'owner' && originalSource.id === documentId
    && originalSource.revision === original.source_revision && typeof originalSource.markdown === 'string',
  'document_format_original_unbound');
  const content = page.getByRole('textbox', {name: 'Document content', exact: true});
  const button = name => page.getByRole('button', {name, exact: true});
  const picker = async (name, choice) => {
    await button(name).click({timeout});
    await page.getByRole('menuitemradio', {name: choice, exact: true}).click({timeout});
  };
  const insert = async name => {
    await button('Insert content').click({timeout});
    await page.getByRole('menuitem', {name, exact: true}).click({timeout});
  };
  const select = async (marker, all = false) => {
    const target = content.getByText(marker, {exact: true});
    await target.click({timeout});
    await target.evaluate((element, allText) => {
      const text = element.firstChild;
      if (!text || text.nodeType !== Node.TEXT_NODE) throw Error('document_format_selection_unbound');
      const length = text.textContent.length;
      window.getSelection().setBaseAndExtent(text, allText ? 0 : length, text, length);
    }, all);
    // ProseMirror reads an external selection on its next input event.
    await page.keyboard.press('Shift');
    check(await page.evaluate(() => String(window.getSelection())) === (all ? marker : ''),
      'document_format_selection_unbound');
  };
  const insertAfter = async (anchor, name) => {
    await select(anchor);
    await page.keyboard.press('Enter');
    await insert(name);
  };

  await page.goto('/artifacts/' + documentId, {waitUntil: 'domcontentloaded', timeout});
  const image = new URL('/fixture-document-format-pixel.png', page.url()).href;
  const origin = new URL(page.url()).origin;
  const servePixel = route => route.fulfill({contentType: 'image/png', body: pixel});
  const matchesSave = request => request.method() === 'PATCH' && request.url() === origin + path;
  let attempted;
  const observeSave = request => {
    if (matchesSave(request)) attempted = request.postDataJSON();
  };
  await page.route(image, servePixel);
  page.on('request', observeSave);
  await withRestoration(async () => {
    await content.fill(markers[0], {timeout});
    await page.keyboard.press('End');
    for (const marker of markers.slice(1)) {
      await page.keyboard.press('Enter');
      await page.keyboard.insertText(marker);
    }
    await select('Fixture heading');
    await picker('Paragraph style', 'Heading 2');
    await select('Fixture paragraph');
    await picker('Paragraph style', 'Heading 1');
    await select('Fixture paragraph');
    await picker('Paragraph style', 'Text');
    await select('Fixture font', true);
    await picker('Font', 'Mono');
    for (const mark of ['Bold', 'Italic', 'Underline']) {
      const label = 'Fixture ' + mark.toLowerCase();
      await select(label, true);
      await button(mark).click({timeout});
      const element = {Bold: 'strong', Italic: 'em', Underline: 'u'}[mark];
      await content.locator(element).filter({hasText: label}).waitFor({timeout});
    }
    for (const [marker, option] of [['bullets', 'Bullet list'], ['numbers', 'Numbered list'],
      ['checklist', 'Checklist'], ['quote', 'Quote'], ['code', 'Code block']]) {
      await select('Fixture ' + marker);
      await insert(option);
    }
    await content.locator('pre').filter({hasText: 'Fixture code'}).first().waitFor({timeout});
    for (const [anchor, option] of [['Mermaid', 'Mermaid diagram'], ['Details', 'Dropdown section'],
      ['Date', 'Date'], ['Divider', 'Divider'], ['Image', 'Image']]) await insertAfter(anchor + ' anchor', option);
    const modal = page.getByRole('dialog', {name: 'Insert image', exact: true});
    const url = modal.getByRole('textbox', {name: 'Image URL', exact: true});
    const submit = modal.getByRole('button', {name: 'Insert image', exact: true});
    await url.fill('ftp://fixture.invalid/pixel.png', {timeout});
    check(await submit.isDisabled(), 'document_format_unsafe_image_accepted');
    await url.fill(image, {timeout});
    await url.press('End', {timeout});
    await poll(() => submit.isEnabled(), Boolean, {timeout, interval: 50, code: 'document_format_safe_image_refused'});
    const description = modal.getByRole('textbox', {name: 'Description', exact: true});
    await description.fill('Fixture pixel', {timeout});
    check(await url.inputValue() === image, 'document_format_image_input_changed');
    await submit.click({timeout});

    await insertAfter('Temporary table anchor', 'Table');
    await content.getByRole('table').getByRole('columnheader').first().click({timeout});
    await insert('Delete table');
    check(await content.getByRole('table').count() === 0, 'document_format_table_not_deleted');
    await insertAfter('Table anchor', 'Table');
    const firstCell = content.getByRole('table').getByRole('columnheader').first();
    await firstCell.click({timeout});
    await page.keyboard.insertText('Fixture cell');
    await insert('Add row');
    await firstCell.click({timeout});
    await insert('Add column');
    await button('Undo').click({timeout});
    check(await content.getByRole('table').getByRole('row').first().getByRole('columnheader').count() === 3,
      'document_format_undo_column_not_removed');
    await button('Redo').click({timeout});
    await verifyRendered(content, image);

    const [response] = await Promise.all([
      page.waitForResponse(value => matchesSave(value.request()), {timeout}), button('Save').click({timeout}),
    ]);
    attempted = response.request().postDataJSON();
    const source = attempted?.markdown;
    check(attempted.expected_revision === original.revision && attempted.title === original.title
      && typeof source === 'string' && source.includes('Fixture cell') && source.includes('```mermaid')
      && await response.request().headerValue('idempotency-key') && response.status() === 200,
    'document_format_save_unbound');
    const saved = await response.json();
    check(saved.id === documentId && saved.revision === original.revision + 1
      && saved.source_revision === saved.revision && saved.sha256 === hash(source),
    'document_format_revision_unbound');
    for (const am of [peer, ingress]) {
      check(equal(await read(am), saved) && equal(await read(am, '/source'),
        {id: documentId, revision: saved.source_revision, markdown: source}), 'document_format_peer_source_changed');
    }
    await page.reload({waitUntil: 'domcontentloaded', timeout});
    await button('Preview').click({timeout});
    check(await content.getAttribute('contenteditable', {timeout}) === 'false', 'document_format_preview_editable');
    await verifyRendered(content, image);
    const diagram = content.getByRole('img', {name: 'Mermaid diagram', exact: true});
    const svg = await diagram.getAttribute('src', {timeout});
    check(svg?.startsWith('data:image/svg+xml;charset=utf-8,')
      && /Start.*Finish/s.test(decodeURIComponent(svg)), 'document_format_mermaid_unbound');
    await button('Edit').click({timeout});
  }, async () => {
    try {
      if (!attempted) return;
      const current = await read(peer);
      const source = await read(peer, '/source');
      if (equal(current, original) && equal(source, originalSource)) return;
      check(current.revision === original.revision + 1 && current.title === original.title
        && source.revision === current.source_revision && source.markdown === attempted.markdown,
      'document_format_restore_foreign_revision');
      const {body: restored} = await user.request(peer, 'PATCH', path, {key: randomUUID(),
        body: {expected_revision: current.revision, title: original.title, markdown: originalSource.markdown}});
      check(restored.id === documentId && restored.revision === current.revision + 1
        && restored.source_revision === restored.revision && restored.sha256 === original.sha256,
      'document_format_restore_receipt_unbound');
      for (const am of [peer, ingress]) {
        check(equal(await read(am), restored) && equal(await read(am, '/source'),
          {...originalSource, revision: restored.source_revision}), 'document_format_original_source_not_restored');
      }
    } finally {
      page.off('request', observeSave);
      await page.unroute(image, servePixel);
    }
  }, recordFailure);
}
