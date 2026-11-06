import { createContext } from 'react';
import type { Cma } from '@chronoai/cma-sdk';

export interface AnswerDeliveryAccess {
  readonly agentId: string;
  readonly viewerId: string;
  readonly client: Pick<
    Cma['agents'],
    'answerDeliverables' | 'readAnswerDeliverable' | 'downloadAnswerDeliverable'
  >;
  readonly authority?: unknown;
}

export const AnswerDeliveryContext = createContext<AnswerDeliveryAccess | undefined>(undefined);

const DELIVERABLE_PREFIX = '<!--cma-deliverable:v1:';
const PARAGRAPH_BOUNDARY = /(\n[ \t\r]*\n)/;

function removePartialMarker(line: string, finalLine: boolean): string {
  const marker = line.indexOf(DELIVERABLE_PREFIX);
  if (marker >= 0) return line.slice(0, marker);
  if (!finalLine) return line;
  for (let length = Math.min(line.length, DELIVERABLE_PREFIX.length - 1); length > 0; length -= 1) {
    if (DELIVERABLE_PREFIX.startsWith(line.slice(-length))) return line.slice(0, -length);
  }
  return line;
}

function maskedOutsideFences(content: string): string {
  const lines = content.split(/(?<=\n)/);
  let fence: { character: string; width: number } | undefined;
  return lines
    .map((line) => {
      const opening = /^\s*(`{3,}|~{3,})(.*?)(\n)?$/.exec(line);
      if (opening) {
        const delimiter = opening[1]!;
        if (!fence) fence = { character: delimiter[0]!, width: delimiter.length };
        else if (
          fence.character === delimiter[0] &&
          delimiter.length >= fence.width &&
          opening[2]!.trim() === ''
        )
          fence = undefined;
        return line.replace(/[^\r\n]/g, ' ');
      }
      if (fence !== undefined) return line.replace(/[^\r\n]/g, ' ');
      return line;
    })
    .join('');
}

function maskCompleteInlineCode(content: string): string {
  return content
    .split(PARAGRAPH_BOUNDARY)
    .map((paragraph, index, paragraphs) =>
      maskParagraphInlineCode(paragraph, index < paragraphs.length - 1),
    )
    .join('');
}

function maskParagraphInlineCode(content: string, complete: boolean): string {
  const chars = content.split('');
  for (let index = 0; index < chars.length;) {
    if (chars[index] !== '`') {
      index += 1;
      continue;
    }
    let width = 1;
    while (chars[index + width] === '`') width += 1;
    let close = index + width;
    while (close < chars.length) {
      if (chars[close] !== '`') {
        close += 1;
        continue;
      }
      let closingWidth = 1;
      while (chars[close + closingWidth] === '`') closingWidth += 1;
      if (closingWidth === width) break;
      close += closingWidth;
    }
    if (close >= chars.length) {
      // An unmatched run is literal once its paragraph has ended.
      if (complete)
        for (let masked = index; masked < index + width; masked += 1) chars[masked] = ' ';
      index += width;
      continue;
    }
    for (let masked = index; masked < close + width; masked += 1) chars[masked] = ' ';
    index = close + width;
  }
  return chars.join('');
}

function emphasisDelimiterState(content: string, delimiter: '*' | '_') {
  const token = delimiter + delimiter;
  let open: number | undefined;
  for (let index = 0; index <= content.length - 2; index += 1) {
    if (content.slice(index, index + 2) !== token) continue;
    const left = content[index - 1] ?? ' ';
    const right = content[index + 2] ?? ' ';
    const leftPunctuation = /[\p{P}\p{S}]/u.test(left);
    const rightPunctuation = /[\p{P}\p{S}]/u.test(right);
    const leftFlanking =
      !/\s/u.test(right) && (!rightPunctuation || /\s/u.test(left) || leftPunctuation);
    const rightFlanking =
      !/\s/u.test(left) && (!leftPunctuation || /\s/u.test(right) || rightPunctuation);
    const canOpen = leftFlanking && (delimiter === '*' || !rightFlanking || leftPunctuation);
    const canClose = rightFlanking && (delimiter === '*' || !leftFlanking || rightPunctuation);
    if (canClose && open !== undefined) open = undefined;
    else if (canOpen) open = index;
    index += 1;
  }
  return open;
}

/** Return the longest Markdown prefix that is safe to parse while a reply is live. */
export function streamingSafePrefix(content: string): string {
  let fence: { character: string; width: number } | undefined;
  const sanitized = content
    .split('\n')
    .map((line, index, lines) => {
      const opening = /^\s*(`{3,}|~{3,})(.*)$/.exec(line);
      if (opening) {
        const delimiter = opening[1]!;
        if (!fence) fence = { character: delimiter[0]!, width: delimiter.length };
        else if (
          fence.character === delimiter[0] &&
          delimiter.length >= fence.width &&
          opening[2]!.trim() === ''
        )
          fence = undefined;
        return line;
      }
      return fence === undefined ? removePartialMarker(line, index === lines.length - 1) : line;
    })
    .join('\n');
  const masked = maskedOutsideFences(sanitized);
  let cut = sanitized.length;
  const link = /!?\[[^\]\n]*(?:\](?:\([^)\n]*)?)?$/.exec(masked);
  if (link) cut = Math.min(cut, link.index);
  const emphasisMasked = maskCompleteInlineCode(masked);
  const finalParagraph = emphasisMasked.split(PARAGRAPH_BOUNDARY).at(-1)!;
  const paragraphOffset = emphasisMasked.length - finalParagraph.length;
  for (const delimiter of ['*', '_'] as const) {
    const unmatched = emphasisDelimiterState(finalParagraph, delimiter);
    if (unmatched !== undefined) cut = Math.min(cut, paragraphOffset + unmatched);
  }
  const danglingEmphasis = /(?:^|[^*])\*(?:$)|(?:^|[^_])_(?:$)/.exec(masked);
  if (danglingEmphasis)
    cut = Math.min(cut, danglingEmphasis.index + danglingEmphasis[0].length - 1);
  const backtick = emphasisMasked.indexOf('`');
  if (backtick >= 0) cut = Math.min(cut, backtick);
  return sanitized.slice(0, cut);
}

export function answerProse(
  content: string,
  streaming = false,
): { content: string; references: number } {
  if (streaming) content = streamingSafePrefix(content);
  if (!content.includes('<!--cma-deliverable:')) return { content, references: 0 };
  let fence: { character: string; width: number } | undefined;
  const markers = new Set<string>();
  const lines = content.split('\n').filter((line) => {
    const opening = /^\s*(`{3,}|~{3,})(.*)$/.exec(line);
    if (opening) {
      const delimiter = opening[1]!;
      if (!fence) fence = { character: delimiter[0]!, width: delimiter.length };
      else if (
        fence.character === delimiter[0] &&
        delimiter.length >= fence.width &&
        opening[2]!.trim() === ''
      )
        fence = undefined;
      return true;
    }
    if (!fence && /^<!--cma-deliverable:v1:[A-Za-z0-9_-]{1,4096}-->\r?$/.test(line)) {
      markers.add(line);
      return false;
    }
    if (!fence && streaming && /^<!--cma-deliverable:v1:[A-Za-z0-9_-]{0,4096}>?$/.test(line))
      return false;
    return true;
  });
  return { content: lines.join('\n'), references: Math.min(markers.size, 32) };
}
