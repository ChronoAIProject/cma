import { memo, useId, useState } from 'react';
import type { ToolCall } from './model';
import { StateDot, type StateDotState } from '../ui/StateDot';
import { IconChevronDownOutline14, IconChevronRightOutline14 } from '../ui/icons';
import { ActivityBody } from './Thinking';
import type { ToolGroup } from './timeline';
import { toolOutcome, toolOutcomeWord, type ToolOutcome } from './toolStatus';
import './ToolCallRows.css';

/** Cancelled ends without asserting success or failure, so like any ended state it draws no dot. */
const DOT: Readonly<Record<ToolOutcome, StateDotState | undefined>> = {
  running: 'ongoing',
  completed: 'done',
  failed: 'error',
  cancelled: undefined,
  unreported: 'neutral',
};

function ToolCallLine({
  tool,
  activeRunId,
}: {
  readonly tool: ToolCall;
  readonly activeRunId: string | undefined;
}) {
  const outcome = toolOutcome(tool, activeRunId);
  const dot = DOT[outcome];
  return (
    <article
      className="agent-event--tool"
      aria-label={`Tool call: ${tool.title}`}
      data-outcome={outcome}
    >
      <div className="tool-call__line">
        <span className="tool-call__dot">
          {dot === undefined ? null : <StateDot state={dot} size={10} />}
        </span>
        <strong>{tool.title}</strong>
        {tool.kind === undefined ? null : <span className="tool-call__kind">{tool.kind}</span>}
        <span className="tool-call__word">{toolOutcomeWord(outcome, tool.status)}</span>
      </div>
      {/* No duration: the public contract reports none, and an absent one is not zero. */}
      <ActivityBody
        label="Details"
        body={tool.detail === undefined ? undefined : { text: tool.detail, truncated: false }}
      />
    </article>
  );
}

/** A lone call sits in the same quiet container as a group, so one and many read alike. */
export function ToolCallRow({
  tool,
  activeRunId,
}: {
  readonly tool: ToolCall;
  readonly activeRunId: string | undefined;
}) {
  return (
    <ToolCallGroup
      group={{
        kind: 'tools',
        id: `tools:${tool.id}`,
        items: [tool],
        failed: toolOutcome(tool, activeRunId) === 'failed',
      }}
      activeRunId={activeRunId}
    />
  );
}

export const ToolCallGroup = memo(function ToolCallGroup({
  group,
  activeRunId,
}: {
  readonly group: ToolGroup;
  readonly activeRunId: string | undefined;
}) {
  // Until the reader chooses, a failure that joins the group opens it.
  const [chosen, setChosen] = useState<boolean>();
  const open = chosen ?? group.failed;
  const rowsId = useId();
  // A settled group repeats no command line: only work still running or calls that
  // failed are worth a reader's glance before they choose to open it.
  const outcomes = group.items.map((tool) => toolOutcome(tool, activeRunId));
  const running = group.items[outcomes.lastIndexOf('running')];
  const failures = outcomes.filter((outcome) => outcome === 'failed').length;
  return (
    <div className="tool-calls" data-group="">
      <button
        type="button"
        className="tool-calls__toggle"
        aria-expanded={open}
        aria-controls={open ? rowsId : undefined}
        onClick={() => setChosen(!open)}
      >
        <span className="tool-calls__chevron" aria-hidden="true">
          {open ? <IconChevronDownOutline14 /> : <IconChevronRightOutline14 />}
        </span>
        <span className="tool-calls__count">
          {group.items.length} tool call{group.items.length === 1 ? '' : 's'}
        </span>
        {/* Spaces between the parts keep the accessible name readable; flex drops them visually. */}
        {running === undefined ? null : (
          <>
            {' '}
            <span className="tool-calls__running">· Running {running.title}</span>
          </>
        )}
        {failures === 0 ? null : (
          <>
            {' '}
            <span className="tool-calls__failed">· {failures} failed</span>
          </>
        )}
      </button>
      {open ? (
        <div className="tool-calls__rows" id={rowsId}>
          {group.items.map((tool) => (
            <ToolCallLine key={tool.id} tool={tool} activeRunId={activeRunId} />
          ))}
        </div>
      ) : null}
    </div>
  );
});
