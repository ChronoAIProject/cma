import type { MouseEvent, ReactNode } from 'react';
import type { PhrasingContent } from 'mdast';

/**
 * The inline run of a block, as the agent wrote it — `Web_Style.md` §18.12.
 *
 * Walked from the parsed tree into React elements. Nothing here is built from
 * a string, so there is no path by which agent text becomes markup.
 *
 * **No colour, ever.** §18.1 reserves the semantic tokens for session and
 * system state, and they describe nothing an agent said — so a link is
 * distinguished by underline and cursor rather than by being blue, which is
 * that exception arriving by habit.
 */

export interface InlineLink {
  readonly href: string;
  readonly external?: boolean;
  readonly onFollow?: () => void;
}

export interface InlineOptions {
  readonly classPrefix?: 'agent' | 'developer';
  readonly resolveLink?: (url: string) => InlineLink | undefined;
  readonly renderImages?: boolean;
  readonly preserveUnknownText?: boolean;
}

/** Where an untrusted external link may point. */
export function safeHttpHref(url: string): string | undefined {
  /*
   * Only the two schemes a reader can act on, and the agent's own text
   * otherwise. `javascript:` and `data:` are the ones that turn a link into
   * an execution, and this content came from an agent running somebody's own
   * code — the least trusted text on the surface.
   */
  const trimmed = url.trim();
  return /^https?:\/\//i.test(trimmed) ? trimmed : undefined;
}

function defaultLink(url: string): InlineLink | undefined {
  const href = safeHttpHref(url);
  return href === undefined ? undefined : { href, external: true };
}

/** Plain words from an inline run, used for document titles and heading slugs. */
export function inlineText(nodes: readonly PhrasingContent[]): string {
  return nodes
    .map((node) => {
      if ('value' in node && typeof node.value === 'string') {
        return node.value;
      }
      if ('children' in node) {
        return inlineText(node.children as readonly PhrasingContent[]);
      }
      return node.type === 'image' ? (node.alt ?? '') : '';
    })
    .join('');
}

export function renderInline(
  nodes: readonly PhrasingContent[],
  options: InlineOptions = {},
): ReactNode[] {
  const prefix = options.classPrefix ?? 'agent';
  const resolveLink = options.resolveLink ?? defaultLink;
  const preserveUnknownText = options.preserveUnknownText ?? true;

  return nodes.map((node, index) => {
    const key = `${node.type}-${String(index)}`;
    switch (node.type) {
      case 'text':
        return node.value;

      case 'inlineCode':
        return (
          <code className={`${prefix}__code`} key={key}>
            {node.value}
          </code>
        );

      case 'strong':
        return <strong key={key}>{renderInline(node.children, options)}</strong>;

      case 'emphasis':
        return <em key={key}>{renderInline(node.children, options)}</em>;

      case 'delete':
        return <del key={key}>{renderInline(node.children, options)}</del>;

      case 'break':
        return <br key={key} />;

      case 'link': {
        const link = resolveLink(node.url);
        if (link === undefined) {
          // Not a link this surface will open. The words survive; only the
          // navigation is withheld.
          return <span key={key}>{renderInline(node.children, options)}</span>;
        }
        return (
          <a
            className={`${prefix}__link`}
            key={key}
            href={link.href}
            target={link.external === true ? '_blank' : undefined}
            /*
             * Untrusted by construction: this came from an agent running the
             * owner's own code. `noopener` denies the opened page a handle on
             * this one; `noreferrer` denies it the address it came from.
             */
            rel={link.external === true ? 'noopener noreferrer' : undefined}
            onClick={
              link.onFollow === undefined
                ? undefined
                : (event: MouseEvent<HTMLAnchorElement>) => {
                    event.preventDefault();
                    link.onFollow?.();
                  }
            }
          >
            {renderInline(node.children, options)}
          </a>
        );
      }

      case 'image': {
        const src = safeHttpHref(node.url);
        if (options.renderImages === false || src === undefined) {
          return <span key={key}>{node.alt ?? ''}</span>;
        }
        /*
         * Drawn, but never allowed to set the column's width: an agent
         * linking a 4000px screenshot should not decide how wide the
         * conversation is.
         */
        return <img className={`${prefix}__image`} key={key} src={src} alt={node.alt ?? ''} />;
      }

      default:
        /*
         * Anything this build does not draw yet — footnote references, raw
         * HTML — comes through as the text it was rather than disappearing.
         * §18.1's rule: an agent that wrote something is owed having written
         * it, whether or not this surface has a shape for it.
         */
        return preserveUnknownText && 'value' in node && typeof node.value === 'string'
          ? node.value
          : null;
    }
  });
}
