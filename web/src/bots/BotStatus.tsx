import type { Bot, BotTest } from '@chronoai/cma-sdk';
import { Alert } from '../ui/Alert';
import { creationStepIndex, setupSteps, operationLabel } from './model';

/** Progress of the consumer's current operation; its state badge sits in the page header. */
export function BotStatus({ bot }: { readonly bot: Bot }) {
  const progress = operationLabel(bot);
  const current = creationStepIndex(bot);
  return (
    <div role="status" aria-live="polite">
      {progress === undefined ? null : <p>{progress}</p>}
      {current === undefined ? null : (
        <ol aria-label="Consumer setup progress">
          {setupSteps(bot).map(([step, label], index) => (
            <li key={step} aria-current={index === current ? 'step' : undefined}>
              {label} · {index < current ? 'Done' : index === current ? 'In progress' : 'Waiting'}
            </li>
          ))}
        </ol>
      )}
      {bot.failure === null ? null : <Alert tone="error">{bot.failure}</Alert>}
    </div>
  );
}
export function BotTestResult({ result }: { readonly result: BotTest | null }) {
  if (result === null) return <p>No test has been recorded.</p>;
  return (
    <div>
      <p>
        Accepted after {result.accepted_after_ms} ms · Provider level {result.level}
      </p>
      <p>{result.first_text}</p>
      <p>
        Tested{' '}
        <time dateTime={result.tested_at}>{new Date(result.tested_at).toLocaleString()}</time>
      </p>
      <p>This synthetic test does not send a message to your channel.</p>
    </div>
  );
}
