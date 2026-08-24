import { CopyKey } from '../chat/CopyKey';
import { blocksOf, type Block, type BlockOptions } from '../chat/blocks';
import { inlineText, renderInline, type InlineLink, type InlineOptions } from '../chat/inline';
import './MarkdownContent.css';

type MarkdownVariant = 'agent' | 'developer';
type HeadingTag = 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6';

interface MarkdownContentProps {
  readonly text: string;
  readonly variant: MarkdownVariant;
  readonly headingOffset?: number;
  readonly headingAnchors?: boolean;
  readonly rawHtml?: BlockOptions['rawHtml'];
  readonly resolveLink?: (url: string) => InlineLink | undefined;
  readonly renderImages?: boolean;
  readonly preserveUnknownInlineText?: boolean;
}

/** A stable, readable fragment for one document heading. */
export function headingSlug(words: string): string {
  const slug = words
    .normalize('NFKD')
    .replace(/[\u0300-\u036f]/g, '')
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9\s-]/g, '')
    .replace(/[\s-]+/g, '-');
  return slug === '' ? 'section' : slug;
}

function headingTag(level: number): HeadingTag {
  return `h${String(Math.min(Math.max(level, 1), 6))}` as HeadingTag;
}

/**
 * The app's one MDAST-to-React renderer.
 *
 * Chat and repository documents supply different policies for trust, heading
 * rank and links, but both walk the same parsed block model and produce React
 * nodes directly. No markdown string is ever treated as HTML.
 */
export function MarkdownContent({
  text,
  variant,
  headingOffset = 0,
  headingAnchors = false,
  rawHtml = 'text',
  resolveLink,
  renderImages = true,
  preserveUnknownInlineText = true,
}: MarkdownContentProps) {
  const slugs = new Map<string, number>();
  const inlineOptions: InlineOptions = {
    classPrefix: variant,
    renderImages,
    preserveUnknownText: preserveUnknownInlineText,
    ...(resolveLink === undefined ? {} : { resolveLink }),
  };

  const renderBlocks = (blocks: readonly Block[]) => (
    <>
      {blocks.map((block, index) => {
        switch (block.kind) {
          case 'code':
            return (
              <div className="codeblock" key={index}>
                <div className="codeblock__head">
                  <span className="codeblock__lang">{block.language}</span>
                  <CopyKey text={block.body} />
                </div>
                <pre className={`${variant}__block`} data-language={block.language}>
                  {block.body}
                </pre>
              </div>
            );
          case 'heading': {
            const Tag = headingTag(block.level + headingOffset);
            const content = renderInline(block.content, inlineOptions);
            if (!headingAnchors) {
              return (
                <Tag className={`${variant}__heading`} data-level={block.level} key={index}>
                  {content}
                </Tag>
              );
            }
            const base = headingSlug(inlineText(block.content));
            const seen = (slugs.get(base) ?? 0) + 1;
            slugs.set(base, seen);
            const id = seen === 1 ? base : `${base}-${String(seen)}`;
            return (
              <Tag className={`${variant}__heading`} data-level={block.level} id={id} key={index}>
                <a className={`${variant}__anchor`} href={`#${id}`}>
                  {content}
                </a>
              </Tag>
            );
          }
          case 'list': {
            const List = block.ordered ? 'ol' : 'ul';
            return (
              <List className={`${variant}__list`} key={index}>
                {block.items.map((item, at) => (
                  <li key={at} data-depth={item.depth === 0 ? undefined : item.depth}>
                    {item.done === undefined ? null : (
                      <input
                        className={`${variant}__check`}
                        type="checkbox"
                        checked={item.done}
                        disabled
                        readOnly
                      />
                    )}
                    {renderInline(item.content, inlineOptions)}
                  </li>
                ))}
              </List>
            );
          }
          case 'quote':
            return (
              <blockquote className={`${variant}__quote`} key={index}>
                {renderBlocks(block.blocks)}
              </blockquote>
            );
          case 'table':
            return (
              <div className={`${variant}__tablewrap`} key={index}>
                <table className={`${variant}__table`}>
                  <thead>
                    <tr>
                      {block.head.map((cell, at) => (
                        <th key={at} scope="col">
                          {renderInline(cell, inlineOptions)}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {block.rows.map((row, at) => (
                      <tr key={at}>
                        {row.map((cell, col) => (
                          <td key={col}>{renderInline(cell, inlineOptions)}</td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            );
          case 'paragraph':
            return (
              <p className={`${variant}__paragraph`} key={index}>
                {renderInline(block.content, inlineOptions)}
              </p>
            );
          case 'rule':
            return <hr className={`${variant}__rule`} key={index} />;
          default: {
            const exhaustive: never = block;
            return exhaustive;
          }
        }
      })}
    </>
  );

  const rootClass = variant === 'agent' ? 'agent' : 'developer__markdown';
  return <div className={rootClass}>{renderBlocks(blocksOf(text, { rawHtml }))}</div>;
}
