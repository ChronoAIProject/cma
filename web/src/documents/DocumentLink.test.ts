import { Editor } from '@tiptap/core';
import { afterEach, expect, it, vi } from 'vitest';
import { documentExtensions } from './DocumentExtensions';

const editors: Editor[] = [];
afterEach(() => editors.splice(0).forEach((editor) => editor.destroy()));
function create(content = '', editable = true, onUpdate = vi.fn()) {
  const editor = new Editor({
    extensions: documentExtensions(),
    content,
    contentType: 'markdown',
    editable,
    onUpdate,
  });
  editors.push(editor);
  return editor;
}
function links(editor: Editor) {
  const spans: { from: number; to: number; text: string; href: string }[] = [];
  editor.state.doc.descendants((node, pos) => {
    const link = node.marks.find((mark) => mark.type.name === 'link');
    if (node.isText && link)
      spans.push({
        from: pos,
        to: pos + node.nodeSize,
        text: node.text!,
        href: String(link.attrs.href),
      });
  });
  return spans;
}
const url = 'https://example.test/en/about/previous-releases';
for (const mode of ['import', 'typing', 'paste'] as const) {
  it.each([...'；，。！？、：（）【】《》〈〉「」『』'])(
    `${mode} ends a bare link at %s and retains the complete suffix`,
    (punctuation) => {
      const prefix = 'Node URL：';
      const text = `${prefix}${url}${punctuation}访问 UTC：2026`;
      const editor = create(mode === 'import' ? text : '');
      if (mode === 'typing') editor.view.dispatch(editor.state.tr.insertText(text + ' '));
      if (mode === 'paste') editor.view.pasteText(text, new Event('paste') as ClipboardEvent);
      expect(editor.state.doc.textContent).toBe(text + (mode === 'typing' ? ' ' : ''));
      expect(links(editor)).toEqual([
        { from: 1 + prefix.length, to: 1 + prefix.length + url.length, text: url, href: url },
      ]);
      expect(links(create(editor.getMarkdown()))).toEqual(links(editor));
    },
  );
}
it('completes a link on CJK punctuation without waiting for a space', () => {
  const editor = create();
  for (const text of [url, '；', '访问']) editor.view.dispatch(editor.state.tr.insertText(text));
  expect(links(editor)).toEqual([{ from: 1, to: 1 + url.length, text: url, href: url }]);
  expect(editor.state.doc.textContent).toBe(`${url}；访问`);
});
it('uses one history event for pasted text and its link ranges', () => {
  const editor = create();
  const text = `${url}；访问`;
  editor.view.pasteText(text, new Event('paste') as ClipboardEvent);
  expect(links(editor)).toHaveLength(1);
  expect(editor.commands.undo()).toBe(true);
  expect(editor.state.doc.textContent).toBe('');
  expect(editor.commands.undo()).toBe(false);
  expect(editor.commands.redo()).toBe(true);
  expect(editor.state.doc.textContent).toBe(text);
  expect(links(editor)).toEqual([{ from: 1, to: 1 + url.length, text: url, href: url }]);
});
it('preserves explicit Unicode links, encoded punctuation and manual links', () => {
  const explicit = 'https://example.test/合法中文路径；访问';
  const encoded = 'https://example.test/path%EF%BC%9B?q=one#two';
  const editor = create(`[说明](${explicit}) ${encoded}；后文`);
  expect(links(editor)).toEqual([
    { from: 1, to: 3, text: '说明', href: explicit },
    { from: 4, to: 4 + encoded.length, text: encoded, href: encoded },
  ]);
  const manual = create('手工链接');
  manual.commands.selectAll();
  expect(manual.commands.setLink({ href: explicit })).toBe(true);
  expect(links(manual)).toEqual([{ from: 1, to: 5, text: '手工链接', href: explicit }]);
  expect(manual.commands.setLink({ href: 'javascript:alert(1)' })).toBe(false);
  expect(links(create(editor.getMarkdown()))).toEqual(links(editor));
});
it('keeps code, escaped URLs, images and HTML links outside bare link rewriting', () => {
  const explicit = `${url}；访问`;
  const editor = create(
    `\`${explicit}\`\n\n\`\`\`text\n${explicit}\n\`\`\`\n\nhttps\\://example.test/path；访问\n\n![图](${explicit})\n\n[旧链接](${explicit})`,
  );
  expect(links(editor)).toEqual([expect.objectContaining({ text: '旧链接', href: explicit })]);
  const code = create('');
  code.commands.toggleCodeBlock();
  code.view.pasteText(`${url}；访问`, new Event('paste') as ClipboardEvent);
  expect(links(code)).toEqual([]);
  const inline = create();
  inline.commands.toggleCode();
  inline.view.dispatch(inline.state.tr.insertText(`${url}；访问 `));
  expect(links(inline)).toEqual([]);
  const html = create();
  html.view.pasteHTML(
    `<p><a href="${explicit}">${explicit}</a></p>`,
    new Event('paste') as ClipboardEvent,
  );
  expect(links(html)).toEqual([
    { from: 1, to: 1 + explicit.length, text: explicit, href: explicit },
  ]);
});
it('does not update on import, Preview toggles, or unrelated formatting', () => {
  const onUpdate = vi.fn();
  const editor = create(`${url}；访问`, false, onUpdate);
  editor.setEditable(true, false);
  editor.setEditable(false, false);
  expect(onUpdate).not.toHaveBeenCalled();
  expect(
    editor.extensionManager.extensions.filter((extension) => extension.name === 'link'),
  ).toHaveLength(1);
  editor.setEditable(true, false);
  editor.commands.selectAll();
  editor.commands.unsetLink();
  editor.commands.toggleBold();
  expect(links(editor)).toEqual([]);
  editor.commands.setTextSelection(editor.state.doc.content.size - 1);
  editor.view.dispatch(editor.state.tr.insertText(' more '));
  expect(links(editor)).toEqual([]);
});
it('links selected labels only for an entire pasted URL', () => {
  const editor = create('label');
  editor.commands.selectAll();
  editor.view.pasteText(url, new Event('paste') as ClipboardEvent);
  expect(links(editor)).toEqual([{ from: 1, to: 6, text: 'label', href: url }]);
  editor.commands.selectAll();
  editor.view.pasteText(`${url}；访问`, new Event('paste') as ClipboardEvent);
  expect(editor.state.doc.textContent).toBe(`${url}；访问`);
  expect(links(editor)).toEqual([{ from: 1, to: 1 + url.length, text: url, href: url }]);
});

for (const mode of ['import', 'typing', 'paste'] as const) {
  it(`${mode} preserves trailing slashes, query, fragments and adjacent URLs`, () => {
    const first = 'https://example.test/path/';
    const second = 'https://other.test/search?q=one&sort=asc#results';
    const text = `${first}；${second}。`;
    const editor = create(mode === 'import' ? text : '');
    if (mode === 'typing') editor.view.dispatch(editor.state.tr.insertText(text));
    if (mode === 'paste') editor.view.pasteText(text, new Event('paste') as ClipboardEvent);
    expect(editor.state.doc.textContent).toBe(text);
    expect(links(editor)).toEqual([
      { from: 1, to: 1 + first.length, text: first, href: first },
      { from: 2 + first.length, to: 2 + first.length + second.length, text: second, href: second },
    ]);
  });
}
it('completes typed URLs on a paragraph break and pastes URLs across paragraphs', () => {
  const editor = create();
  editor.view.dispatch(editor.state.tr.insertText(url));
  expect(links(editor)).toEqual([]);
  editor.commands.splitBlock();
  expect(links(editor)).toEqual([{ from: 1, to: 1 + url.length, text: url, href: url }]);
  const pasted = create();
  pasted.view.pasteText(`${url}\n${url}；访问`, new Event('paste') as ClipboardEvent);
  expect(links(pasted)).toEqual([
    { from: 1, to: 1 + url.length, text: url, href: url },
    { from: 3 + url.length, to: 3 + 2 * url.length, text: url, href: url },
  ]);
});
it('keeps ASCII punctuation, email, domain, and invalid scheme behavior', () => {
  const editor = create();
  editor.view.pasteText(
    '(https://example.test/path), example.com reader@example.com javascript:alert(1)',
    new Event('paste') as ClipboardEvent,
  );
  expect(links(editor).map(({ text, href }) => ({ text, href }))).toEqual([
    { text: 'https://example.test/path', href: 'https://example.test/path' },
    { text: 'example.com', href: 'http://example.com' },
    { text: 'reader@example.com', href: 'mailto:reader@example.com' },
  ]);
  expect(create('https://example.test/path.').getHTML()).toContain(
    'href="https://example.test/path"',
  );
  const invalid = create();
  invalid.view.dispatch(invalid.state.tr.insertText('example.com1 '));
  expect(links(invalid)).toEqual([]);
});
it('preserves automatic link ranges across formatting nodes and inline atoms', () => {
  const editor = create('[date value="2026-11-20"] ');
  editor.commands.setTextSelection(editor.state.doc.content.size - 1);
  editor.view.dispatch(editor.state.tr.insertText(`${url}；访问`));
  expect(links(editor)).toEqual([{ from: 3, to: 3 + url.length, text: url, href: url }]);
  const formatted = create('**Node URL：**');
  formatted.commands.setTextSelection(formatted.state.doc.content.size - 1);
  formatted.view.dispatch(formatted.state.tr.insertText(`${url}；访问`));
  expect(links(formatted)).toEqual([{ from: 10, to: 10 + url.length, text: url, href: url }]);
});

it('pastes into selected code and preserves explicitly linked HTML over a selection', () => {
  const editor = create('`replace`');
  editor.commands.setTextSelection({ from: 1, to: 8 });
  editor.view.pasteText(url, new Event('paste') as ClipboardEvent);
  expect(editor.state.doc.textContent).toBe(url);
  expect(links(editor)).toEqual([]);
  const html = create('replace');
  html.commands.selectAll();
  const href = 'https://example.test/explicit；访问';
  html.view.pasteHTML(`<p><a href="${href}">${url}</a></p>`, new Event('paste') as ClipboardEvent);
  expect(html.state.doc.textContent).toBe(url);
  expect(links(html)).toEqual([{ from: 1, to: 1 + url.length, text: url, href }]);
});
it('keeps unqualified IP addresses and escaped URLs unlinked during input', () => {
  const editor = create();
  editor.view.dispatch(editor.state.tr.insertText('127.0.0.1 https\\://example.test/path；访问 '));
  expect(links(editor)).toEqual([]);
});
it('waits for a newly typed delimiter when inserting before existing text', () => {
  const editor = create('Node URL：');
  editor.commands.setTextSelection(1);
  for (const character of url) editor.view.dispatch(editor.state.tr.insertText(character));
  expect(links(editor)).toEqual([]);
  editor.view.dispatch(editor.state.tr.insertText('；访问 '));
  expect(links(editor)).toEqual([{ from: 1, to: 1 + url.length, text: url, href: url }]);
  expect(editor.state.doc.textContent).toBe(`${url}；访问 Node URL：`);
});
it('preserves explicit hrefs when their labels are different bare URLs', () => {
  const explicit = 'https://example.test/明确目标；访问';
  const editor = create(`[${url}](${explicit})`);
  expect(links(editor)).toEqual([{ from: 1, to: 1 + url.length, text: url, href: explicit }]);
  expect(links(create(editor.getMarkdown()))).toEqual(links(editor));
});
