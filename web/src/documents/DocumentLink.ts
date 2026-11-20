import Link, { isAllowedUri } from '@tiptap/extension-link';
import { Lexer } from 'marked';
import { bareLinkPrefix } from './documentLinkBoundary';
import { documentAutolink } from './documentAutolink';

export const DocumentLink = Link.extend({
  markdownTokenizer: {
    name: 'documentBareLink',
    level: 'inline',
    start: (source) => source.search(/(?:https?:\/\/|ftp:\/\/|www\.)/i),
    tokenize(source) {
      if (!/^(?:https?:\/\/|ftp:\/\/|www\.)/i.test(source)) return undefined;
      const token = Lexer.lexInline(bareLinkPrefix(source), { gfm: true })[0];
      return token?.type === 'link' ? token : undefined;
    },
  },
  parseMarkdown(token, helpers) {
    const content = helpers.parseInline(token.tokens ?? []).map((node) => ({
      ...node,
      ...(node.marks ? { marks: node.marks.filter((mark) => mark.type !== 'link') } : {}),
    }));
    return helpers.applyMark('link', content, { href: token.href, title: token.title || null });
  },
  addPasteRules: () => [],
  addProseMirrorPlugins() {
    const { protocols, defaultProtocol, isAllowedUri: validate, shouldAutoLink } = this.options;
    return [
      ...(this.parent?.() ?? []),
      documentAutolink({
        editor: this.editor,
        type: this.type,
        defaultProtocol,
        shouldAutoLink,
        validate: (href) =>
          validate(href, {
            defaultValidate: (url) => !!isAllowedUri(url, protocols),
            protocols,
            defaultProtocol,
          }),
      }),
    ];
  },
}).configure({
  autolink: false,
  linkOnPaste: false,
  openOnClick: 'whenNotEditable',
  HTMLAttributes: { rel: 'noopener noreferrer', target: '_blank' },
});
