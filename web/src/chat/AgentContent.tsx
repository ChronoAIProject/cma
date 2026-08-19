import type { ReactNode } from 'react';
import { CopyKey } from './CopyKey';

/**
 * Agent-emitted content, rendered as the agent meant it — `Web_Style.md` §1
 * and §18.1.
 *
 * "The frame is styled and the content is not." Code blocks, lists and inline
 * formatting are structure the agent wrote, so they are drawn as structure;
 * **no colour is ever applied**, because §2 reserves the semantic tokens for
 * session and system state and they describe nothing an agent said. That rules
 * out syntax highlighting, a tint per role, and colouring a word the agent
 * happened to use.
 *
 * Hand-written rather than taken as a dependency. What is rendered is a closed
 * set — fenced code, inline code, list items, headings, and paragraphs — and a
 * markdown library brings a configuration surface, a sanitiser to keep correct,
 * and a much larger answer to the question "what can an agent put on this
 * page?" than this needs. Everything not in the set is drawn as the text it
 * was, which is the safe direction: an agent that wrote something this does
 * not parse has its words shown rather than swallowed.
 *
 * Nothing here produces HTML from a string. React escapes what it renders, so
 * an agent cannot emit markup that becomes markup — which matters more here
 * than anywhere else in the product, because this content came from a coding
 * agent executing the user's own untrusted code (§9).
 */

interface AgentContentProps {
  readonly text: string;
}

/** One item in a list, and how deep the agent put it. */
export interface ListItem {
  readonly text: string;
  /**
   * Nesting depth, from the indentation the agent wrote.
   *
   * §18.12: a plan with sub-steps flattened to one level is a different plan.
   */
  readonly depth: number;
}

/** One block of what the agent wrote. */
type Block =
  | { readonly kind: 'code'; readonly language: string; readonly body: string }
  | { readonly kind: 'heading'; readonly level: number; readonly body: string }
  | { readonly kind: 'list'; readonly items: readonly ListItem[]; readonly ordered: boolean }
  | { readonly kind: 'quote'; readonly body: string }
  | { readonly kind: 'table'; readonly head: readonly string[]; readonly rows: readonly string[][] }
  | { readonly kind: 'paragraph'; readonly body: string };

const FENCE = /^```(\w*)\s*$/;
const HEADING = /^(#{1,6})\s+(.*)$/;
const BULLET = /^(\s*)[-*]\s+(.*)$/;
const NUMBERED = /^(\s*)\d+[.)]\s+(.*)$/;
const QUOTE = /^>\s?(.*)$/;
/** A row of a pipe table: at least one `|` with something either side. */
const ROW = /^\s*\|(.+)\|\s*$/;
/** The rule under a table's head — dashes and optional alignment colons. */
const RULE = /^\s*\|[\s:|-]+\|\s*$/;

/** One row's cells, trimmed, from the text between its outer pipes. */
function cellsOf(line: string): string[] {
  return (ROW.exec(line)?.[1] ?? '').split('|').map((cell) => cell.trim());
}

/**
 * How deep an item sits, from the spaces in front of it.
 *
 * Two spaces per level is the common convention and four is the other; both
 * land on the same depth here because the depth is what matters and the exact
 * indent an agent chose does not. Capped, so a pathological indent cannot push
 * a list off the reading column.
 */
function depthOf(indent: string): number {
  return Math.min(Math.floor(indent.replace(/\t/g, '  ').length / 2), 5);
}

/**
 * Split what the agent wrote into blocks.
 *
 * A fence that never closes takes the rest of the message, which is what a
 * reader would expect and is better than dropping everything after it.
 */
export function blocksOf(text: string): Block[] {
  const lines = text.split('\n');
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  let items: ListItem[] = [];
  let ordered = false;
  let quoted: string[] = [];

  const flushParagraph = () => {
    if (paragraph.length > 0) {
      blocks.push({ kind: 'paragraph', body: paragraph.join('\n') });
      paragraph = [];
    }
  };
  const flushList = () => {
    if (items.length > 0) {
      blocks.push({ kind: 'list', items, ordered });
      items = [];
    }
  };
  const flushQuote = () => {
    if (quoted.length > 0) {
      blocks.push({ kind: 'quote', body: quoted.join('\n') });
      quoted = [];
    }
  };
  const flush = () => {
    flushParagraph();
    flushList();
    flushQuote();
  };

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i] ?? '';
    const fence = FENCE.exec(line);
    if (fence) {
      flush();
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !FENCE.test(lines[i] ?? '')) {
        body.push(lines[i] ?? '');
        i += 1;
      }
      blocks.push({ kind: 'code', language: fence[1] ?? '', body: body.join('\n') });
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      flush();
      blocks.push({ kind: 'heading', level: heading[1]?.length ?? 1, body: heading[2] ?? '' });
      continue;
    }

    // A pipe table, recognised only where the head is followed by its rule.
    // Without that check a line of prose containing pipes becomes a table.
    if (ROW.test(line) && RULE.test(lines[i + 1] ?? '')) {
      flush();
      const head = cellsOf(line);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && ROW.test(lines[i] ?? '')) {
        rows.push(cellsOf(lines[i] ?? ''));
        i += 1;
      }
      i -= 1;
      blocks.push({ kind: 'table', head, rows });
      continue;
    }

    const quote = QUOTE.exec(line);
    if (quote) {
      flushParagraph();
      flushList();
      quoted.push(quote[1] ?? '');
      continue;
    }
    flushQuote();

    const bullet = BULLET.exec(line);
    const numbered = NUMBERED.exec(line);
    if (bullet ?? numbered) {
      flushParagraph();
      const nextOrdered = numbered !== null;
      if (items.length > 0 && nextOrdered !== ordered) {
        flushList();
      }
      ordered = nextOrdered;
      items.push({
        text: numbered?.[2] ?? bullet?.[2] ?? '',
        depth: depthOf(numbered?.[1] ?? bullet?.[1] ?? ''),
      });
      continue;
    }

    flushList();
    if (line.trim() === '') {
      flushParagraph();
    } else {
      paragraph.push(line);
    }
  }

  flush();
  return blocks;
}

/**
 * Inline code and emphasis — §18.1: "Lists, headings and emphasis take the
 * scale §4 defines."
 *
 * Emphasis was required from the day §18 was written and never rendered, so
 * `**do not**` reached the reader as four asterisks around a word. An agent
 * emphasising something is an agent saying it matters, and printing its
 * punctuation is the surface refusing to hear that.
 *
 * Code is taken out first and its contents are left alone: `` `a ** b` `` is
 * an expression, not emphasis, and a pass that ran the other way round would
 * quietly rewrite what the agent wrote. Weight and slant carry it, never
 * colour — §18.1 forbids colour on agent content outright.
 */
export function inline(text: string): ReactNode[] {
  const out: ReactNode[] = [];
  text.split(/(`[^`]+`)/).forEach((part, index) => {
    if (part.startsWith('`') && part.endsWith('`') && part.length > 2) {
      out.push(
        <code className="agent__code" key={`c${String(index)}`}>
          {part.slice(1, -1)}
        </code>,
      );
      return;
    }
    out.push(...links(part, index));
  });
  return out;
}

/**
 * Strong and emphatic runs, in the two spellings agents actually use.
 *
 * `**` and `__` are strong; single `*` and `_` are emphatic. Underscores are
 * matched only at a word boundary, because `snake_case_names` are ordinary
 * text an agent writes constantly and turning their middles italic would
 * corrupt an identifier the reader may be about to copy.
 */
/**
 * A link, and nothing decided about where it points — §18.12.
 *
 * Underline and cursor distinguish it, never colour: §18.1 forbids colour on
 * agent content without exception, and a blue link is that exception arriving
 * by habit. The href is what the agent wrote, on §9's precedent for the
 * terminal's OSC 8 sequences — this surface does not decide where an agent's
 * link goes.
 *
 * `noreferrer` and `noopener` because the target is untrusted by construction:
 * this content came from a coding agent running the owner's own code.
 */
const LINK = /\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g;

function links(text: string, seed: number): ReactNode[] {
  const out: ReactNode[] = [];
  let at = 0;
  for (const match of text.matchAll(LINK)) {
    const start = match.index;
    if (start > at) {
      out.push(...emphasis(text.slice(at, start), seed));
    }
    out.push(
      <a
        className="agent__link"
        key={`l${String(seed)}-${String(start)}`}
        href={match[2]}
        target="_blank"
        rel="noopener noreferrer"
      >
        {match[1]}
      </a>,
    );
    at = start + match[0].length;
  }
  if (at < text.length) {
    out.push(...emphasis(text.slice(at), seed));
  }
  return out;
}

function emphasis(text: string, seed: number): ReactNode[] {
  // Typed as ReactNode[] rather than inferred: `flatMap` in the caller needs a
  // uniform element type, and a mix of strings and elements infers as neither.
  const pattern = /(\*\*[^*]+\*\*|__[^_]+__|\*[^*]+\*|\b_[^_]+_\b)/g;
  const parts: ReactNode[] = text.split(pattern).map((part, index): ReactNode => {
    const key = `e${String(seed)}-${String(index)}`;
    if (
      (part.startsWith('**') && part.endsWith('**')) ||
      (part.startsWith('__') && part.endsWith('__'))
    ) {
      return <strong key={key}>{part.slice(2, -2)}</strong>;
    }
    if (
      (part.startsWith('*') && part.endsWith('*') && part.length > 2) ||
      (part.startsWith('_') && part.endsWith('_') && part.length > 2)
    ) {
      return <em key={key}>{part.slice(1, -1)}</em>;
    }
    return part;
  });
  return parts;
}

export function AgentContent({ text }: AgentContentProps) {
  return (
    <div className="agent">
      {blocksOf(text).map((block, index) => {
        switch (block.kind) {
          case 'code':
            return (
              /*
               * A wrapper, a header, and the `<pre>` that keeps the scrolling
               * — §18.11.
               *
               * The header cannot live inside the `<pre>`: that element owns
               * the horizontal overflow, and a sticky child of a horizontally
               * scrolling box slides away sideways instead of staying put. So
               * the frame moves out and the `<pre>` keeps its one job.
               *
               * The label is the fence's own info string, verbatim. Uppercased
               * by `text-transform`, which is typography; mapping `py` onto
               * "Python" would be naming, and §18.2 and §18.10 both forbid this
               * client inventing a set the kernel did not use. A fence with no
               * info string draws no label rather than an invented one.
               */
              <div className="codeblock" key={index}>
                <div className="codeblock__head">
                  <span className="codeblock__lang">{block.language ?? ''}</span>
                  <CopyKey text={block.body} />
                </div>
                <pre className="agent__block" data-language={block.language}>
                  {block.body}
                </pre>
              </div>
            );
          case 'heading': {
            /*
             * A heading element, not a styled paragraph. §18.1 says headings
             * take §4's scale, and a `<p>` gives a long answer no outline at
             * all — a screen reader cannot jump between the sections of an
             * agent's plan, which is exactly the document where jumping is
             * worth something.
             *
             * Offset by one so the agent's top level lands at `h2`: the page
             * owns `h1`, and an agent that starts at `#` must not compete
             * with it for the document's title.
             */
            const Tag = `h${String(Math.min(block.level + 1, 6))}` as 'h2';
            return (
              <Tag className="agent__heading" data-level={block.level} key={index}>
                {inline(block.body)}
              </Tag>
            );
          }
          case 'list': {
            /*
             * Nesting kept — §18.12. Carried as a depth on each item and drawn
             * as an indent rather than rebuilt into nested `<ul>`s: the agent's
             * markers are what they are, and re-deriving a tree from
             * indentation invents structure where the agent only wrote
             * spacing. A plan with sub-steps flattened to one level is a
             * different plan, which is the thing this prevents.
             */
            const List = block.ordered ? 'ol' : 'ul';
            return (
              <List className="agent__list" key={index}>
                {block.items.map((item, at) => (
                  <li key={at} data-depth={item.depth === 0 ? undefined : item.depth}>
                    {inline(item.text)}
                  </li>
                ))}
              </List>
            );
          }
          case 'quote':
            return (
              <blockquote className="agent__quote" key={index}>
                {inline(block.body)}
              </blockquote>
            );
          case 'table':
            /*
             * §18.12: the structure the agent wrote it in, scrolling inside
             * its own frame rather than widening the reading column — the same
             * rule a code block follows, for the same reason.
             */
            return (
              <div className="agent__tablewrap" key={index}>
                <table className="agent__table">
                  <thead>
                    <tr>
                      {block.head.map((cell, at) => (
                        <th key={at} scope="col">
                          {inline(cell)}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {block.rows.map((row, at) => (
                      <tr key={at}>
                        {row.map((cell, col) => (
                          <td key={col}>{inline(cell)}</td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            );
          case 'paragraph':
            return (
              <p className="agent__paragraph" key={index}>
                {inline(block.body)}
              </p>
            );
          default: {
            const exhaustive: never = block;
            return exhaustive;
          }
        }
      })}
    </div>
  );
}
