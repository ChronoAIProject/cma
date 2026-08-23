import { CopyKey } from './CopyKey';
import { blocksOf, type Block } from './blocks';
import { renderInline } from './inline';

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
 * The parsing is micromark's, not ours — see `markdown.ts` for why. What is
 * *drawn* is still a small closed set, and anything outside it comes through
 * as the text it was: an agent that wrote something is owed having written it,
 * whether or not this surface has a shape for it.
 */

interface AgentContentProps {
  readonly text: string;
}

export function AgentContent({ text }: AgentContentProps) {
  return (
    <div className="agent">
      <Blocks blocks={blocksOf(text)} />
    </div>
  );
}

/** The block renderer, reused wherever blocks nest — a quote, for one. */
function Blocks({ blocks }: { readonly blocks: readonly Block[] }) {
  return (
    <>
      {blocks.map((block, index) => {
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
                {renderInline(block.content)}
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
                    {/*
                      A task list keeps its boxes. They are the whole point of
                      the notation, and an agent's checklist rendered without
                      them is a list of sentences.
                      Disabled: this is a record of what the agent decided,
                      not a control — ticking it here would change nothing and
                      imply it had.
                    */}
                    {item.done === undefined ? null : (
                      <input
                        className="agent__check"
                        type="checkbox"
                        checked={item.done}
                        disabled
                        readOnly
                      />
                    )}
                    {renderInline(item.content)}
                  </li>
                ))}
              </List>
            );
          }
          case 'quote':
            /*
             * A quote holds blocks, not a line: an agent quoting a list or a
             * code fence wrote a list or a code fence, and flattening it to
             * one paragraph would be this surface deciding what it quoted.
             */
            return (
              <blockquote className="agent__quote" key={index}>
                <Blocks blocks={block.blocks} />
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
                          {renderInline(cell)}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {block.rows.map((row, at) => (
                      <tr key={at}>
                        {row.map((cell, col) => (
                          <td key={col}>{renderInline(cell)}</td>
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
                {renderInline(block.content)}
              </p>
            );
          case 'rule':
            return <hr className="agent__rule" key={index} />;
          default: {
            const exhaustive: never = block;
            return exhaustive;
          }
        }
      })}
    </>
  );
}
