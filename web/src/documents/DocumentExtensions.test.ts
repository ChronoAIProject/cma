import { expect, it } from 'vitest';
import { MarkdownManager } from '@tiptap/markdown';
import { documentExtensions, safeDocumentImage } from './DocumentExtensions';

it('round-trips Markdown structure, lists, tables, images, diagrams, fonts and date nodes', () => {
  const manager = new MarkdownManager({ extensions: documentExtensions() });
  const markdown = `# Title\n\n**Bold** and *italic*.\n\n- [x] Done\n- [ ] Next\n\n| Item | Value |\n| --- | --- |\n| A | 1 |\n\n![Result](https://example.test/result.png)\n\n> Quote\n\n---\n\n[font family="serif"]Serif **text**[/font] [date value="2026-10-22"]\n\n\`\`\`mermaid\nflowchart LR\n A --> B\n\`\`\`\n`;
  const parsed = manager.parse(markdown);
  const restored = manager.parse(manager.serialize(parsed));
  expect(restored).toEqual(parsed);
  const data = JSON.stringify(restored);
  for (const type of [
    'heading',
    'bold',
    'italic',
    'taskList',
    'table',
    'image',
    'blockquote',
    'horizontalRule',
    'documentFont',
    'date',
    'codeBlock',
  ])
    expect(data).toContain(`"type":"${type}"`);
  expect(data).toContain('mermaid');
});

it('blocks executable and unsupported image URL schemes', () => {
  for (const url of [
    'javascript:alert(1)',
    'data:image/svg+xml;base64,PHN2Zz4=',
    'file:///etc/passwd',
    '//example.test/tracker',
  ])
    expect(safeDocumentImage(url)).toBeUndefined();
  expect(safeDocumentImage('https://example.test/image.png')).toBeDefined();
  const manager = new MarkdownManager({ extensions: documentExtensions() });
  expect(JSON.stringify(manager.parse('![unsafe](javascript:alert)'))).not.toContain(
    '"type":"image"',
  );
});

it('round-trips collapsible details without silently removing their contents', () => {
  const manager = new MarkdownManager({ extensions: documentExtensions() });
  const document = {
    type: 'doc',
    content: [
      {
        type: 'details',
        attrs: { open: true },
        content: [
          { type: 'detailsSummary', content: [{ type: 'text', text: 'Summary' }] },
          {
            type: 'detailsContent',
            content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Body' }] }],
          },
        ],
      },
    ],
  };
  const restored = manager.parse(manager.serialize(document));
  expect(JSON.stringify(restored)).toContain('Summary');
  expect(JSON.stringify(restored)).toContain('Body');
  expect(restored.content?.[0]?.content?.[1]?.type).toBe('detailsContent');
  expect(JSON.stringify(restored.content?.[0]?.content?.[1])).toContain('Body');
});

it('imports exact bare link ranges without changing suffixes or explicit historical hrefs', () => {
  const manager = new MarkdownManager({ extensions: documentExtensions() });
  const url = 'https://example.test/en/about/previous-releases';
  const markdown = `Node URL：${url}；访问 UTC：2026\n\n[既有链接](${url}；访问)`;
  const parsed = manager.parse(markdown);
  expect(parsed.content?.[0]?.content).toEqual([
    { type: 'text', text: 'Node URL：' },
    { type: 'text', text: url, marks: [{ type: 'link', attrs: { href: url, title: null } }] },
    { type: 'text', text: '；访问 UTC：2026' },
  ]);
  expect(parsed.content?.[1]?.content).toEqual([
    {
      type: 'text',
      text: '既有链接',
      marks: [{ type: 'link', attrs: { href: `${url}；访问`, title: null } }],
    },
  ]);
  expect(manager.parse(manager.serialize(parsed))).toEqual(parsed);
});
