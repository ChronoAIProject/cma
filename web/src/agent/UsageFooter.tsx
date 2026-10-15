import type { AgentUsage } from '../api/types';
import { BoundedReading } from './Resources';

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
            {usage === undefined
              ? 'not reported'
              : `${count(usage.inputTokens)} input · ${count(usage.outputTokens)} output`}
          </span>
        </div>
      </div>
    </section>
  );
}
