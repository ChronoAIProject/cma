import type { PhrasingContent, Root, RootContent, TableRow } from 'mdast';
import { parse } from './markdown';

/**
 * What an agent wrote, as blocks this surface knows how to draw.
 *
 * A thin mapping from the parsed tree onto the shapes the renderer already
 * has. It exists so the renderer keeps switching on a small closed set — the
 * property §18.1 depends on, since everything outside that set is drawn as
 * the text it was rather than dropped.
 */

export interface ListItem {
  readonly content: readonly PhrasingContent[];
  /**
   * Nesting depth, as the agent nested it.
   *
   * §18.12: a plan with sub-steps flattened to one level is a different plan.
   */
  readonly depth: number;
  /**
   * Whether the item carried a checkbox, and whether it was ticked.
   *
   * `undefined` for an ordinary item. A task list rendered without its boxes
   * loses the one thing it was written to carry.
   */
  readonly done?: boolean;
}

export type Block =
  | { readonly kind: 'code'; readonly language: string; readonly body: string }
  | {
      readonly kind: 'heading';
      readonly level: number;
      readonly content: readonly PhrasingContent[];
    }
  | { readonly kind: 'list'; readonly items: readonly ListItem[]; readonly ordered: boolean }
  | { readonly kind: 'quote'; readonly blocks: readonly Block[] }
  | {
      readonly kind: 'table';
      readonly head: readonly (readonly PhrasingContent[])[];
      readonly rows: readonly (readonly (readonly PhrasingContent[])[])[];
    }
  | { readonly kind: 'paragraph'; readonly content: readonly PhrasingContent[] }
  | { readonly kind: 'rule' };

/** Flatten one list, carrying the depth its nesting had. */
function itemsOf(node: Extract<RootContent, { type: 'list' }>, depth: number): ListItem[] {
  const items: ListItem[] = [];
  for (const item of node.children) {
    const own: PhrasingContent[] = [];
    const nested: ListItem[] = [];
    for (const child of item.children) {
      if (child.type === 'paragraph') {
        own.push(...child.children);
      } else if (child.type === 'list') {
        nested.push(...itemsOf(child, depth + 1));
      }
    }
    items.push({
      content: own,
      depth,
      ...(item.checked === null || item.checked === undefined ? {} : { done: item.checked }),
    });
    items.push(...nested);
  }
  return items;
}

function cellsOf(row: TableRow): readonly (readonly PhrasingContent[])[] {
  return row.children.map((cell) => cell.children);
}

export interface BlockOptions {
  /** What to do with a raw HTML block. React never receives it as markup. */
  readonly rawHtml?: 'text' | 'omit';
}

function blockOf(node: RootContent, options: BlockOptions): Block | undefined {
  switch (node.type) {
    case 'code':
      return { kind: 'code', language: node.lang ?? '', body: node.value };
    case 'heading':
      return { kind: 'heading', level: node.depth, content: node.children };
    case 'list':
      return { kind: 'list', items: itemsOf(node, 0), ordered: node.ordered === true };
    case 'blockquote':
      return {
        kind: 'quote',
        blocks: node.children
          .map((child) => blockOf(child, options))
          .filter((block) => block !== undefined),
      };
    case 'table':
      return {
        kind: 'table',
        head: node.children[0] === undefined ? [] : cellsOf(node.children[0]),
        rows: node.children.slice(1).map(cellsOf),
      };
    case 'paragraph':
      return { kind: 'paragraph', content: node.children };
    case 'thematicBreak':
      return { kind: 'rule' };
    case 'html':
      if (options.rawHtml === 'omit') {
        return undefined;
      }
      /*
       * Raw HTML is shown as the text it was, never as markup — §18.1 and §9.
       * This came from an agent executing somebody's own code, so it is the
       * least trusted text on the surface: it is carried as a `text` node,
       * which React escapes, and the words survive.
       *
       * Dropping it instead would be the worse failure of the two. An agent
       * that wrote something is owed having written it, and silently
       * swallowing a line is how a reader ends up trusting a transcript that
       * is missing part of what happened.
       */
      return { kind: 'paragraph', content: [{ type: 'text', value: node.value }] };
    default:
      /*
       * Nothing this build draws yet — raw HTML, a footnote definition. It is
       * skipped as a block rather than rendered as markup, and its words are
       * not lost: micromark leaves unparsed text as text, which reaches the
       * reader through the paragraph around it.
       */
      return undefined;
  }
}

export function blocksOf(text: string, options: BlockOptions = {}): Block[] {
  const tree: Root = parse(text);
  return tree.children.map((node) => blockOf(node, options)).filter((block) => block !== undefined);
}
