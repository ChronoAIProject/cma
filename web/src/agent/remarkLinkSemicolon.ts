import type { Parent, Root, RootContent } from 'mdast';
import type { Plugin } from 'unified';

const semicolon = '；';

function splitBareLinks(parent: Parent, source: string) {
  parent.children = parent.children.flatMap((child): RootContent[] => {
    if (child.type !== 'link') {
      if ('children' in child) splitBareLinks(child, source);
      return [child];
    }

    const boundary = child.url.indexOf(semicolon);
    const [label] = child.children;
    if (boundary < 0 || child.children.length !== 1 || label?.type !== 'text') return [child];

    const { value } = label;
    if (child.url !== value && child.url !== `http://${value}`) return [child];

    // GFM can also generate links from text nodes without source positions.
    // For positioned links, matching the raw span excludes [label](url) and <url>.
    const start = child.position?.start.offset;
    const end = child.position?.end.offset;
    if (start !== undefined && end !== undefined && source.slice(start, end) !== value) {
      return [child];
    }

    const suffix = child.url.slice(boundary);
    return [
      {
        type: 'link',
        url: child.url.slice(0, boundary),
        children: [
          {
            type: 'text',
            value: value.slice(0, -suffix.length),
          },
        ],
      },
      { type: 'text', value: suffix },
    ];
  });
}

export const remarkLinkSemicolon: Plugin<[], Root> = () => (tree, file) => {
  const source = file.toString().replace(/^\uFEFF/u, '');
  if (source.includes(semicolon)) splitBareLinks(tree, source);
};
