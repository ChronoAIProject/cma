import { memo, useMemo, useState } from 'react';
import type { ToolCall } from '../model';
import { toolOutcome, toolOutcomeWord } from '../toolStatus';
import { Button } from '../../ui/Button';

const PAGE_SIZE = 50;

export const OperationOutput = memo(function OperationOutput({
  toolCalls,
  activeRunId,
  active,
}: {
  readonly toolCalls?: readonly ToolCall[];
  readonly activeRunId?: string;
  readonly active: boolean;
}) {
  const [before, setBefore] = useState<number>();
  const operations = useMemo(
    () => (active ? (toolCalls?.filter((tool) => tool.kind === 'execute') ?? []) : []),
    [toolCalls, active],
  );
  const end = Math.min(before ?? operations.length, operations.length);
  const start = Math.max(0, end - PAGE_SIZE);
  const turns = useMemo(() => {
    const grouped = new Map<string | undefined, ToolCall[]>();
    for (const tool of operations.slice(start, end)) {
      const turn = grouped.get(tool.runId);
      if (turn) turn.push(tool);
      else grouped.set(tool.runId, [tool]);
    }
    return [...grouped];
  }, [operations, start, end]);
  if (!active) return null;
  if (!toolCalls) return <p role="status">Loading recorded operations…</p>;
  if (!operations.length) return <p>No command operations reported for this Agent.</p>;
  return (
    <>
      <p>Read-only recorded operations. No command input is available.</p>
      <nav aria-label="Operation pages">
        <Button size="sm" disabled={start === 0} onClick={() => setBefore(start)}>
          Older operations
        </Button>
        <Button
          size="sm"
          disabled={end === operations.length}
          onClick={() =>
            setBefore(end + PAGE_SIZE >= operations.length ? undefined : end + PAGE_SIZE)
          }
        >
          Newer operations
        </Button>
      </nav>
      <p role="status">
        Showing operations {start + 1}–{end} of {operations.length}
      </p>
      {turns.map(([turn, tools]) => (
        <section
          key={turn ?? 'unreported'}
          aria-label={turn ? `Turn ${turn}` : 'Turn not reported'}
        >
          <h3>{turn ? `Turn ${turn}` : 'Turn not reported'}</h3>
          {tools.map((tool) => (
            <article key={tool.id} aria-label={`Operation: ${tool.title}`}>
              <h4>{tool.title}</h4>
              <p>{toolOutcomeWord(toolOutcome(tool, activeRunId), tool.status)}</p>
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
