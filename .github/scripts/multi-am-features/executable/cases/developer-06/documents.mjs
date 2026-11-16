import {
  define,
  setup,
  open,
  api,
  button,
  pick,
  visible,
  contains,
  absent,
  capture,
  enc,
  hash,
  until,
  peer,
} from './helpers.mjs';
const content = (ctx) => ctx.state.b.page.getByLabel('Document content', { exact: true });
const docpath = (ctx) => `/api/v1/documents/${enc(ctx.state.f.documentId)}`;
async function enter(ctx, scenario) {
  const { page, origin, f } = await setup(ctx, scenario);
  await page.goto(origin + '/artifacts/' + enc(f.documentId));
  await visible(ctx, content(ctx), 'document_loaded');
  ctx.assert((await content(ctx).getAttribute('contenteditable')) === 'true', 'document_editable');
}
async function insert(page, name) {
  await button(page, 'Insert content').click();
  await page.getByRole('menuitem', { name, exact: true }).click();
}
async function paragraph(ctx, text) {
  const e = content(ctx);
  await e.press('ControlOrMeta+End');
  await e.press('Enter');
  await e.press('Enter');
  await pick(ctx.state.b.page, 'Paragraph style', 'Text');
  await e.pressSequentially(text);
}
export const cases = [
  define('documents.format', [
    async (ctx) => {
      await enter(ctx, 'document-format');
      const page = ctx.state.b.page;
      for (const style of ['Text', 'Heading 1', 'Heading 2', 'Heading 3']) {
        await paragraph(ctx, 'format ' + style);
        await pick(page, 'Paragraph style', style);
        await contains(ctx, button(page, 'Paragraph style'), style, 'paragraph_style_selected');
      }
      for (const font of ['Sans', 'Serif', 'Mono']) {
        await paragraph(ctx, 'font ' + font);
        await content(ctx).press('Home');
        await content(ctx).press('Shift+End');
        await pick(page, 'Font', font);
        await content(ctx).press('ArrowRight');
      }
      for (const mark of ['Bold', 'Italic', 'Underline']) {
        await paragraph(ctx, 'mark ' + mark);
        await content(ctx).press('Home');
        await content(ctx).press('Shift+End');
        await button(page, mark).click();
        await content(ctx).press('ArrowRight');
      }
      for (const tag of ['h1', 'h2', 'h3', 'strong', 'em', 'u'])
        ctx.assert((await content(ctx).locator(tag).count()) > 0, 'format_mark_rendered');
    },
    async (ctx) => {
      const page = ctx.state.b.page;
      for (const name of ['Bullet list', 'Numbered list', 'Checklist', 'Quote', 'Code block']) {
        await paragraph(ctx, 'structure ' + name);
        await insert(page, name);
        ctx.assert((await content(ctx).innerText()) !== '', 'structured_content_present');
      }
      for (const name of ['Mermaid diagram', 'Dropdown section', 'Date', 'Divider']) {
        await paragraph(ctx, 'before ' + name);
        await insert(page, name);
      }
      await paragraph(ctx, 'approved image');
      await insert(page, 'Image');
      const dialog = page.getByRole('dialog', { name: 'Insert image' });
      await dialog.getByLabel('Image URL', { exact: true }).fill('javascript:alert(1)');
      ctx.assert(await button(dialog, 'Insert image').isDisabled(), 'unsafe_image_refused');
      await dialog.getByLabel('Image URL', { exact: true }).fill(ctx.state.f.approvedImageUrl);
      await dialog.getByLabel('Description', { exact: true }).fill('local approved image');
      await button(dialog, 'Insert image').click();
      await visible(
        ctx,
        content(ctx).getByRole('img', { name: 'local approved image', exact: true }),
        'approved_image_rendered',
      );
    },
    async (ctx) => {
      const page = ctx.state.b.page;
      await paragraph(ctx, 'table start');
      await insert(page, 'Table');
      const table = content(ctx).locator('table');
      ctx.assert((await table.locator('tr').count()) === 3, 'table_three_rows');
      await table.locator('th,td').first().click();
      await insert(page, 'Add row');
      await insert(page, 'Add column');
      ctx.assert(
        (await table.locator('tr').count()) === 4 &&
          (await table.locator('tr').first().locator('th,td').count()) === 4,
        'table_added_row_column',
      );
      await insert(page, 'Delete table');
      await absent(ctx, content(ctx).locator('table'), 'offered_table_removal');
      await button(page, 'Undo').click();
      ctx.assert((await content(ctx).locator('table').count()) === 1, 'table_removal_undo');
    },
    async (ctx) => {
      const page = ctx.state.b.page;
      await paragraph(ctx, 'undo redo marker');
      const before = await content(ctx).innerText();
      await button(page, 'Undo').click();
      ctx.assert((await content(ctx).innerText()) !== before, 'undo_changes_content');
      await button(page, 'Redo').click();
      await contains(ctx, content(ctx), 'undo redo marker', 'redo_restores_content');
      const r = await capture(page, 'PATCH', docpath(ctx), () => button(page, 'Save').click());
      ctx.assert(r.status === 200 && r.body.id === ctx.state.f.documentId, 'formatted_document_saved');
      ctx.state.savedMarkdown = r.request.markdown;
      await button(page, 'Preview').click();
      ctx.assert((await content(ctx).getAttribute('contenteditable')) === 'false', 'preview_readonly');
    },
    async (ctx) => {
      const r = await api(ctx, docpath(ctx) + '/source');
      ctx.assert(r.body.markdown === ctx.state.savedMarkdown, 'format_exact_saved_roundtrip');
      await ctx.state.b.switchAM(peer(ctx));
      await ctx.state.b.page.reload();
      await button(ctx.state.b.page, 'Preview').click();
      for (const selector of [
        'h1',
        'h2',
        'h3',
        'strong',
        'em',
        'u',
        'ul',
        'ol',
        'blockquote',
        'table',
        'time',
        'hr',
        'img[alt="local approved image"]',
      ])
        ctx.assert((await content(ctx).locator(selector).count()) > 0, 'format_roundtrip_nodes');
      await visible(
        ctx,
        ctx.state.b.page.getByRole('img', { name: 'Mermaid diagram', exact: true }),
        'mermaid_preview_rendered',
      );
      await absent(ctx, content(ctx).locator('script, iframe, [onerror], [onload]'), 'document_safe_preview');
      await ctx.observe('document_source', async () => ({ sha256: hash(r.body.markdown) }));
    },
  ]),
  define('documents.preview-outline', [
    (ctx) => enter(ctx, 'document-outline'),
    async (ctx) => {
      await button(ctx.state.b.page, 'Preview').click();
      ctx.assert((await content(ctx).getAttribute('contenteditable')) === 'false', 'preview_no_editor');
      await visible(
        ctx,
        ctx.state.b.page.getByRole('toolbar', { name: 'Document navigation' }),
        'reading_toolbar',
      );
    },
    async (ctx) => {
      const page = ctx.state.b.page;
      await button(page, 'On this page').click();
      const outline = page.getByRole('navigation', { name: 'On this page' });
      for (const heading of ctx.state.f.headings) {
        await button(outline, heading).click();
        const node = content(ctx).getByRole('heading', { name: heading, exact: true });
        await until(async () =>
          node.evaluate((e) => {
            const r = e.getBoundingClientRect();
            return r.top >= 0 && r.top < innerHeight;
          }),
        );
        ctx.assert(await node.isVisible(), 'outline_target_visible');
      }
      await contains(ctx, content(ctx), ctx.state.f.codeMarker, 'code_view');
      await visible(ctx, page.getByRole('img', { name: 'Mermaid diagram' }), 'diagram_view');
    },
    async (ctx) => {
      const f = ctx.state.f;
      const b = await open(ctx, 'outsider');
      await b.page.goto(b.origin + '/artifacts/' + enc(f.documentId));
      const read = b.page.getByLabel('Document content', { exact: true });
      await visible(ctx, read, 'readonly_document_open');
      ctx.assert((await read.getAttribute('contenteditable')) === 'false', 'readonly_no_editable_content');
      for (const name of ['Save', 'Edit', 'Bold', 'Italic', 'Insert content'])
        await absent(ctx, button(b.page, name), 'readonly_no_mutation_controls');
      const source = await api(ctx, docpath(ctx) + '/source');
      ctx.assert(hash(source.body.markdown) === f.sourceSha256, 'preview_saved_source');
      const denied = await api(ctx, docpath(ctx), {
        actor: 'outsider',
        method: 'PATCH',
        body: { expected_revision: f.revision, markdown: 'must be refused' },
        key: f.deniedKey,
        allowed: [403, 404],
      });
      ctx.assert([403, 404].includes(denied.status), 'readonly_server_mutation_denied');
    },
  ]),
];
