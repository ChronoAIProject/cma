import type { ReactNode } from 'react';
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

/** Where a link may point. */
function safeHref(url: string): string | undefined {
  /*
   * Only the two schemes a reader can act on, and the agent's own text
   * otherwise. `javascript:` and `data:` are the ones that turn a link into
   * an execution, and this content came from an agent running somebody's own
   * code — the least trusted text on the surface.
   */
  const trimmed = url.trim();
  return /^https?:\/\//i.test(trimmed) ? trimmed : undefined;
}

export function renderInline(nodes: readonly PhrasingContent[]): ReactNode[] {
  return nodes.map((node, index) => {
    const key = `${node.type}-${String(index)}`;
    switch (node.type) {
      case 'text':
        return node.value;

      case 'inlineCode':
        return (
          <code className="agent__code" key={key}>
            {node.value}
          </code>
        );

      case 'strong':
        return <strong key={key}>{renderInline(node.children)}</strong>;

      case 'emphasis':
        return <em key={key}>{renderInline(node.children)}</em>;

      case 'delete':
        return <del key={key}>{renderInline(node.children)}</del>;

      case 'break':
        return <br key={key} />;

      case 'link': {
        const href = safeHref(node.url);
        if (href === undefined) {
          // Not a link this surface will open. The words survive; only the
          // navigation is withheld.
          return <span key={key}>{renderInline(node.children)}</span>;
        }
        return (
          <a
            className="agent__link"
            key={key}
            href={href}
            target="_blank"
            /*
             * Untrusted by construction: this came from an agent running the
             * owner's own code. `noopener` denies the opened page a handle on
             * this one; `noreferrer` denies it the address it came from.
             */
            rel="noopener noreferrer"
          >
            {renderInline(node.children)}
          </a>
        );
      }

      case 'image': {
        const src = safeHref(node.url);
        if (src === undefined) {
          return <span key={key}>{node.alt ?? ''}</span>;
        }
        /*
         * Drawn, but never allowed to set the column's width: an agent
         * linking a 4000px screenshot should not decide how wide the
         * conversation is.
         */
        return <img className="agent__image" key={key} src={src} alt={node.alt ?? ''} />;
      }

      default:
        /*
         * Anything this build does not draw yet — footnote references, raw
         * HTML — comes through as the text it was rather than disappearing.
         * §18.1's rule: an agent that wrote something is owed having written
         * it, whether or not this surface has a shape for it.
         */
        return 'value' in node && typeof node.value === 'string' ? node.value : null;
    }
  });
}
