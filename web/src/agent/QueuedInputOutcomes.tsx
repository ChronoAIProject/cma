import { Button } from '../ui/Button';
import { UserMessage } from './UserMessage';
import { queuedOutcomeText, type QueuedInputOutcome } from './transport/queuedCommandOutcomes';
import './QueuedInputOutcomes.css';
import { ExpiredMessageActions } from './ExpiredMessageActions';

export function QueuedInputOutcomes({
  items,
  onRefresh,
  error,
  onRestore,
  restoreDisabled,
}: {
  readonly error?: string;
  readonly items: readonly QueuedInputOutcome[];
  readonly onRefresh: () => void;
  readonly onRestore?: (text: string) => void;
  readonly restoreDisabled?: boolean;
}) {
  const cancelled = items.filter((item) => item.terminal && item.kind === 'cancelled');
  const shown = items.filter((item) => !item.terminal || item.kind !== 'cancelled');
  return (
    <>
      {items.length ? (
        <section className="queued-input-outcomes" aria-label="Message status">
          {shown.length ? (
            <OutcomeList items={shown} onRestore={onRestore} restoreDisabled={restoreDisabled} />
          ) : null}
          {cancelled.length ? (
            <details>
              <summary>Cancelled messages ({cancelled.length})</summary>
              <OutcomeList items={cancelled} />
            </details>
          ) : null}
          {shown.some((item) => !item.terminal) ? (
            <Button size="sm" onClick={onRefresh}>
              Refresh status
            </Button>
          ) : null}
        </section>
      ) : null}
      {error ? (
        <p className="queued-messages__error" role="status">
          {error}{' '}
          <Button size="sm" onClick={onRefresh}>
            Refresh queue
          </Button>
        </p>
      ) : null}
    </>
  );
}

function OutcomeList({
  items,
  onRestore,
  restoreDisabled,
}: {
  readonly items: readonly QueuedInputOutcome[];
  readonly onRestore?: (text: string) => void;
  readonly restoreDisabled?: boolean;
}) {
  return (
    <ol className="queued-input-outcomes__list">
      {items.map((item) => (
        <li key={item.commandId}>
          <article aria-label={item.input ? 'You' : 'Previously submitted message'}>
            {item.input ? (
              <UserMessage content={item.input} />
            ) : (
              <p>Previously submitted message</p>
            )}
            <p role="status">{queuedOutcomeText[item.kind]}</p>
            {item.kind === 'expired' && item.terminal ? (
              <ExpiredMessageActions
                input={item.input}
                onRestore={onRestore}
                disabled={restoreDisabled}
              />
            ) : null}
          </article>
        </li>
      ))}
    </ol>
  );
}
