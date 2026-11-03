import { memo } from 'react';
import type { ConversationState } from '../model';
import { toolOutcome, toolOutcomeWord } from '../toolStatus';

export const OperationOutput = memo(function OperationOutput({
  conversation,
}: {
  readonly conversation?: ConversationState;
}) {
  if (!conversation) return <p role="status">Loading recorded operations…</p>;
  const operations = conversation.toolCalls.filter((tool) => tool.kind === 'execute');
  if (!operations.length) return <p>No command operations reported for this Agent.</p>;
  const turns = [...new Set(operations.map((tool) => tool.runId))];
  return (
    <>
      <p>Read-only recorded operations. No command input is available.</p>
      {turns.map((turn) => (
        <section
          key={turn ?? 'unreported'}
          aria-label={turn ? `Turn ${turn}` : 'Turn not reported'}
        >
          <h3>{turn ? `Turn ${turn}` : 'Turn not reported'}</h3>
          {operations
            .filter((tool) => tool.runId === turn)
            .map((tool) => (
              <article key={tool.id} aria-label={`Operation: ${tool.title}`}>
                <h4>{tool.title}</h4>
                <p>{toolOutcomeWord(toolOutcome(tool, conversation.activeRunId), tool.status)}</p>
                {tool.output ? (
                  <>
                    <pre>{tool.output.text}</pre>
                    {tool.output.truncated ? <p>Output truncated</p> : null}
                  </>
                ) : (
                  <p>Output not reported</p>
                )}
              </article>
            ))}
        </section>
      ))}
    </>
  );
});
