import { ObservedCount } from './ObservedCount';
import { VisibleTokens } from './VisibleTokens';
import type { VisibleText } from './tokenEstimate';

export function TurnUsage({
  usage,
  animateInitial = true,
  visibleText,
  visibleAnchor,
}: {
  readonly usage?: { readonly inputTokens: number; readonly outputTokens: number };
  readonly animateInitial?: boolean;
  readonly visibleText?: readonly VisibleText[];
  readonly visibleAnchor?: readonly VisibleText[];
}) {
  return (
    <span className="thinking__usage" aria-live="off">
      {usage === undefined ? (
        <span title="This turn has not reported token counts. Some agents report totals only after a model response finishes.">
          Turn input unavailable · {visibleText === undefined ? 'output unavailable' : null}
        </span>
      ) : (
        <>
          <span aria-label={`Turn input tokens: ${usage.inputTokens.toLocaleString('en')}`}>
            <ObservedCount value={usage.inputTokens} animateInitial={animateInitial} /> input
          </span>
          {' · '}
          <span aria-label={`Turn output tokens: ${usage.outputTokens.toLocaleString('en')}`}>
            <ObservedCount value={usage.outputTokens} animateInitial={animateInitial} /> output
          </span>
        </>
      )}
      {visibleText === undefined ? null : (
        <VisibleTokens parts={visibleText} anchor={visibleAnchor} reported={usage} />
      )}
    </span>
  );
}
