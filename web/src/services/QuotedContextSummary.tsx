import type { ChannelActivityEvent } from './useChannelActivity';

export function QuotedContextSummary({
  context,
}: {
  readonly context: ChannelActivityEvent['quoted_context'];
}) {
  if (!context) return null;
  return (
    <p aria-label="Reply context">
      {context.relation === 'reply' ? 'Reply context' : 'Native context'}:{' '}
      {context.text_available
        ? context.text_truncated
          ? 'quoted excerpt available (truncated)'
          : 'quoted text available'
        : context.content_kinds.length
          ? 'quoted content described'
          : 'native message reference only'}
      .{context.content_kinds.length ? ` Content: ${context.content_kinds.join(', ')}.` : ''}
    </p>
  );
}
