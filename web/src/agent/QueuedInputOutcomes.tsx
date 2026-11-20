import { Button } from '../ui/Button';
import { UserMessage } from './UserMessage';
import { queuedOutcomeText, type QueuedInputOutcome } from './transport/queuedCommandOutcomes';
import './QueuedInputOutcomes.css';

export function QueuedInputOutcomes({
  items,
  onRefresh,
  error,
}: {
  readonly error?: string;
  readonly items: readonly QueuedInputOutcome[];
  readonly onRefresh: () => void;
}) {
  return (
    <>
      {items.length ? (
        <section className="queued-input-outcomes" aria-label="Message status">
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
                </article>
              </li>
            ))}
          </ol>
          {items.some((item) => !item.terminal) ? (
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
