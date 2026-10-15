import { ObservedCount } from './ObservedCount';

export function TurnUsage({
  usage,
}: {
  readonly usage?: { readonly inputTokens: number; readonly outputTokens: number };
}) {
  return (
    <span className="thinking__usage" aria-live="off">
      {usage === undefined ? (
        <span title="This turn has not reported token counts. Some existing agents report totals only after a turn finishes.">
          Turn input unavailable · output unavailable
        </span>
      ) : (
        <>
          <span aria-label={`Turn input tokens: ${usage.inputTokens.toLocaleString('en')}`}>
            <ObservedCount value={usage.inputTokens} /> input
          </span>
          {' · '}
          <span aria-label={`Turn output tokens: ${usage.outputTokens.toLocaleString('en')}`}>
            <ObservedCount value={usage.outputTokens} /> output
          </span>
        </>
      )}
    </span>
  );
}
