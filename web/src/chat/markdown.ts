import { fromMarkdown } from 'mdast-util-from-markdown';
import { gfmFromMarkdown } from 'mdast-util-gfm';
import { gfm } from 'micromark-extension-gfm';
import type { Root } from 'mdast';

/**
 * Reading what an agent wrote — `docs/Web_Style.md` §18.12.
 *
 * The parser is micromark, not one of ours. This surface used to hand-roll
 * one out of regular expressions, and the failures were the ones hand-rolled
 * parsers always have: `![alt](url)` rendered as a link with a stray
 * exclamation mark in front of it, a task list lost its boxes, and anything
 * the regexes did not anticipate came through as punctuation.
 *
 * Agent output is the one content on this surface the product does not
 * control, so it is exactly where a parser has to be right about input nobody
 * predicted — which is an argument for a well-tested one rather than against
 * the dependency.
 *
 * **Parsing only.** Nothing here produces HTML, and no rendered node comes
 * from a string: the tree is walked into React elements, so there is no path
 * by which agent text becomes markup.
 */

/** Parse into an mdast tree, with GitHub's extensions. */
export function parse(text: string): Root {
  return fromMarkdown(text, {
    extensions: [gfm()],
    mdastExtensions: [gfmFromMarkdown()],
  });
}
