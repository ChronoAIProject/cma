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

/** Inline code, and nothing else. Everything else is the text it was. */
function inline(text: string): ReactNode[] {
  return text.split(/(`[^`]+`)/).map((part, index) =>
    part.startsWith('`') && part.endsWith('`') && part.length > 2 ? (
      <code className="agent__code" key={index}>
        {part.slice(1, -1)}
      </code>
    ) : (
      part
    ),
  );
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
          case 'heading':
            return (
              <p className="agent__heading" data-level={block.level} key={index}>
                {inline(block.body)}
              </p>
            );
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
