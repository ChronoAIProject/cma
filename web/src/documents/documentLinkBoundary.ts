import { find } from 'linkifyjs';

export const documentLinkDelimiter = /[\s；，。！？、：（）【】《》〈〉「」『』]/u;

export function bareLinkPrefix(text: string): string {
  const end = text.search(documentLinkDelimiter);
  return end < 0 ? text : text.slice(0, end);
}

export function bareDocumentLinks(text: string, defaultProtocol: string) {
  const links: ReturnType<typeof find> = [];
  let offset = 0;
  while (offset < text.length) {
    const part = bareLinkPrefix(text.slice(offset));
    for (const link of find(part, { defaultProtocol })) {
      links.push({ ...link, start: offset + link.start, end: offset + link.end });
    }
    offset += part.length + 1;
  }
  return links;
}
