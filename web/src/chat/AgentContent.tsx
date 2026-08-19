import type { ReactNode } from 'react';

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

/** One block of what the agent wrote. */
type Block =
  | { readonly kind: 'code'; readonly language: string; readonly body: string }
  | { readonly kind: 'heading'; readonly level: number; readonly body: string }
  | { readonly kind: 'list'; readonly items: readonly string[]; readonly ordered: boolean }
  | { readonly kind: 'paragraph'; readonly body: string };

const FENCE = /^```(\w*)\s*$/;
const HEADING = /^(#{1,6})\s+(.*)$/;
const BULLET = /^\s*[-*]\s+(.*)$/;
const NUMBERED = /^\s*\d+[.)]\s+(.*)$/;

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
  let items: string[] = [];
  let ordered = false;

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
  const flush = () => {
    flushParagraph();
    flushList();
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

    const bullet = BULLET.exec(line);
    const numbered = NUMBERED.exec(line);
    if (bullet ?? numbered) {
      flushParagraph();
      const nextOrdered = numbered !== null;
      if (items.length > 0 && nextOrdered !== ordered) {
        flushList();
      }
      ordered = nextOrdered;
      items.push(numbered?.[1] ?? bullet?.[1] ?? '');
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
    out.push(...emphasis(part, index));
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
              // The language is carried as data rather than used to colour
              // anything: a client that highlighted would be styling content.
              <pre className="agent__block" data-language={block.language} key={index}>
                {block.body}
              </pre>
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
          case 'list':
            return block.ordered ? (
              <ol className="agent__list" key={index}>
                {block.items.map((item, at) => (
                  <li key={at}>{inline(item)}</li>
                ))}
              </ol>
            ) : (
              <ul className="agent__list" key={index}>
                {block.items.map((item, at) => (
                  <li key={at}>{inline(item)}</li>
                ))}
              </ul>
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
