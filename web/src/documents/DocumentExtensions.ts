import { Mark, Node, createInlineMarkdownSpec } from '@tiptap/core';
import StarterKit from '@tiptap/starter-kit';
import { Markdown } from '@tiptap/markdown';
import { TableKit } from '@tiptap/extension-table';
import Image from '@tiptap/extension-image';
import TaskList from '@tiptap/extension-task-list';
import TaskItem from '@tiptap/extension-task-item';
import { Details, DetailsContent, DetailsSummary } from '@tiptap/extension-details';
import { DocumentCodeBlock } from './DocumentCodeBlock';

export function safeDocumentImage(value: string): string | undefined {
  if (
    /^data:image\/(png|jpeg|gif|webp);base64,[a-z0-9+/=]+$/i.test(value) &&
    value.length <= 900_000
  )
    return value;
  try {
    const url = new URL(value);
    if (url.protocol === 'https:' || url.protocol === 'http:') return value;
  } catch {
    /* Not a supported image. */
  }
  return undefined;
}
const font = (value: unknown) =>
  ['sans', 'serif', 'mono'].includes(String(value)) ? String(value) : 'sans';
export const DocumentFont = Mark.create({
  name: 'documentFont',
  addAttributes: () => ({
    family: {
      default: 'sans',
      parseHTML: (element) => font(element.dataset.font),
      renderHTML: (attributes) => ({ 'data-font': font(attributes.family) }),
    },
  }),
  parseHTML: () => [{ tag: 'span[data-font]' }],
  renderHTML: ({ HTMLAttributes }) => ['span', HTMLAttributes, 0],
  markdownTokenName: 'documentFont',
  markdownTokenizer: {
    name: 'documentFont',
    level: 'inline',
    start: (source) => source.indexOf('[font '),
    tokenize(source, _tokens, lexer) {
      const match = /^\[font family="(sans|serif|mono)"\]([\s\S]*?)\[\/font\]/.exec(source);
      if (!match) return undefined;
      return {
        type: 'documentFont',
        raw: match[0],
        family: match[1],
        tokens: lexer.inlineTokens(match[2] ?? ''),
      };
    },
  },
  parseMarkdown: (token, helpers) =>
    helpers.applyMark('documentFont', helpers.parseInline(token.tokens ?? []), {
      family: font(token.family),
    }),
  renderMarkdown: (node, helpers) =>
    `[font family="${font(node.attrs?.family)}"]${helpers.renderChildren(node)}[/font]`,
});
export const DocumentDate = Node.create({
  name: 'date',
  group: 'inline',
  inline: true,
  atom: true,
  addAttributes: () => ({
    value: { default: '', parseHTML: (element) => element.getAttribute('datetime') },
  }),
  parseHTML: () => [{ tag: 'time[datetime]' }],
  renderHTML: ({ node }) => [
    'time',
    { datetime: String(node.attrs.value) },
    String(node.attrs.value),
  ],
  ...createInlineMarkdownSpec({
    nodeName: 'date',
    selfClosing: true,
    allowedAttributes: ['value'],
  }),
});
const SafeImage = Image.extend({
  parseHTML: () => [
    {
      tag: 'img[src]',
      getAttrs: (element) => (safeDocumentImage(element.getAttribute('src') ?? '') ? {} : false),
    },
  ],
  renderHTML: ({ node }) => [
    'img',
    {
      src: safeDocumentImage(String(node.attrs.src)),
      alt: String(node.attrs.alt ?? ''),
      title: String(node.attrs.title ?? ''),
      loading: 'lazy',
      referrerpolicy: 'no-referrer',
    },
  ],
  parseMarkdown: (token, helpers) =>
    safeDocumentImage(String(token.href ?? ''))
      ? helpers.createNode('image', { src: token.href, title: token.title, alt: token.text })
      : helpers.createTextNode(String(token.text ?? 'Image')),
});
export function documentExtensions() {
  return [
    StarterKit.configure({
      codeBlock: false,
      link: {
        openOnClick: 'whenNotEditable',
        autolink: true,
        HTMLAttributes: { rel: 'noopener noreferrer', target: '_blank' },
      },
    }),
    DocumentCodeBlock,
    Markdown,
    TableKit.configure({ table: { resizable: true } }),
    SafeImage.configure({ allowBase64: true }),
    TaskList,
    TaskItem.configure({ nested: true, HTMLAttributes: { 'data-type': 'taskItem' } }),
    Details.configure({ persist: true }),
    DetailsSummary,
    DetailsContent,
    DocumentFont,
    DocumentDate,
  ];
}
