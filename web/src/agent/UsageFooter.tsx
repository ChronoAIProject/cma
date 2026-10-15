import type { AgentUsage } from '../api/types';
import { BoundedReading } from './Resources';
import { ObservedCount } from './ObservedCount';

const count = (value: number) => new Intl.NumberFormat('en').format(value);

export function UsageFooter({ usage }: { readonly usage?: AgentUsage }) {
  return (
    <section className="agent-instruments" aria-label="Agent usage">
      <div className="agent-instruments__usage">
        <BoundedReading
          label="Context"
          value={usage?.contextUsed}
          limit={usage?.contextWindow}
          format={count}
        />
        <div className="resources__row">
          <span
            className="resources__figure"
            title="Cumulative token usage for this Agent, including previous turns"
          >
            Total tokens ·{' '}
            {usage === undefined ? (
              'input unavailable · output unavailable'
            ) : (
              <>
                <span aria-label={`Total input tokens: ${count(usage.inputTokens)}`}>
                  <ObservedCount value={usage.inputTokens} animateInitial={false} /> input
                </span>
                {' · '}
                <span aria-label={`Total output tokens: ${count(usage.outputTokens)}`}>
                  <ObservedCount value={usage.outputTokens} animateInitial={false} /> output
                </span>
              </>
            )}
          </span>
        </div>
      </div>
    </section>
  );
}
