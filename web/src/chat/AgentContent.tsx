import { MarkdownContent } from '../markdown/MarkdownContent';

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
  return <MarkdownContent text={text} variant="agent" headingOffset={1} />;
}
