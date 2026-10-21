import { useId, useState } from 'react';
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

/** A lone call keeps the same frame as a group, so one and many read as the same kind of thing. */
export function ToolCallRow({
  tool,
  activeRunId,
}: {
  readonly tool: ToolCall;
  readonly activeRunId: string | undefined;
}) {
  return (
    <div className="tool-calls">
      <ToolCallLine tool={tool} activeRunId={activeRunId} />
    </div>
  );
}

export function ToolCallGroup({
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
  const latest = group.items.at(-1)!;
  const latestWord = toolOutcomeWord(toolOutcome(latest, activeRunId), latest.status);
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
        <span className="tool-calls__count">{group.items.length} tool calls</span>{' '}
        <span className="tool-calls__latest">
          Latest: {latest.title} · {latestWord}
        </span>
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
}
