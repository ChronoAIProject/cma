import { screenOperation, screenOperationTime } from './screenOperations';
import { memo, useContext, useId, useState } from 'react';
import type { ToolCall } from './model';
import {
  IconChevronDownOutline14,
  IconChevronRightOutline14,
  IconRightUpOutline16,
} from '../ui/icons';
import type { ToolGroup } from './timeline';
import { toolOutcome, toolOutcomeWord } from './toolStatus';
import { ComputerContext } from './environment/context';
import { toolTimingLabel } from './toolTiming';
import { toolCategory, summarizeTools, conciseToolError } from './toolSummary';
import { ToolIcon } from './ToolIcon';
import './ToolCallRows.css';

function ToolCallLine({
  tool,
  activeRunId,
}: {
  readonly tool: ToolCall;
  readonly activeRunId?: string;
}) {
  const [open, setOpen] = useState(false);
  const computer = useContext(ComputerContext);
  const outcome = toolOutcome(tool, activeRunId);
  const category = toolCategory(tool);
  const title = tool.title.trim() ? tool.title : category === 'command' ? 'Command' : 'Tool call';
  const screen = screenOperation(tool);
  const duration = toolTimingLabel(tool);
  const detailsId = useId();
  return (
    <article
      className="agent-event--tool"
      aria-label={`Tool call: ${title}`}
      data-outcome={outcome}
    >
      {screen ? (
        <div className="tool-call__line tool-call__screen">
          <span className="tool-call__icon" aria-hidden="true">
            <ToolIcon kind={screen.action} />
          </span>
          <span className="tool-call__literal" title={title}>
            {screen.description}
          </span>
          {duration ? (
            <span className="tool-call__duration" title="Recorded tool interval">
              {duration}
            </span>
          ) : null}
          {screen.at ? (
            <time dateTime={screen.at}>{screenOperationTime(screen.at)}</time>
          ) : (
            <span aria-label="Time not reported">—</span>
          )}
        </div>
      ) : (
        <div className="tool-call__line">
          <button
            type="button"
            className="tool-call__expand"
            aria-expanded={open}
            aria-controls={open ? detailsId : undefined}
            onClick={() => setOpen(!open)}
          >
            <span className="tool-call__icon" aria-hidden="true">
              <ToolIcon kind={category} />
            </span>
            <span className="tool-call__literal" title={title}>
              {title}
            </span>
            {duration === undefined ? null : (
              <span className="tool-call__duration" title="Recorded tool interval">
                {duration}
              </span>
            )}
            <span
              className="tool-call__outcome"
              role="img"
              aria-label={toolOutcomeWord(outcome, tool.status)}
              title={toolOutcomeWord(outcome, tool.status)}
            >
              {outcome === 'completed'
                ? '✓'
                : outcome === 'failed'
                  ? '!'
                  : outcome === 'cancelled'
                    ? '■'
                    : '·'}
            </span>
            {category === 'command' ? null : (
              <span className="tool-call__terminal" aria-hidden="true" />
            )}
          </button>
          {category === 'command' ? (
            <button
              type="button"
              className="tool-call__terminal"
              aria-label="View in Operations"
              title="View in Operations"
              disabled={!computer}
              onClick={(event) => {
                event.stopPropagation();
                computer?.selectTerminal(tool.id);
              }}
            >
              <IconRightUpOutline16 />
            </button>
          ) : null}
        </div>
      )}
      {outcome === 'failed' ? <p className="tool-call__error">{conciseToolError(tool)}</p> : null}
      {open ? (
        <div id={detailsId} className="tool-call__details">
          <pre>{tool.detail ?? title}</pre>
          {tool.output ? (
            <>
              <pre aria-label={category === 'edit' ? 'Recorded diff' : 'Recorded output'}>
                {category === 'edit'
                  ? tool.output.text.split('\n').map((line, index) => (
                      <span
                        key={index}
                        className={
                          line.startsWith('+') && !line.startsWith('+++')
                            ? 'tool-call__added'
                            : line.startsWith('-') && !line.startsWith('---')
                              ? 'tool-call__removed'
                              : undefined
                        }
                      >
                        {line}
                        {'\n'}
                      </span>
                    ))
                  : tool.output.text}
              </pre>
              {tool.output.truncated ? <p>Output truncated</p> : null}
            </>
          ) : null}
        </div>
      ) : null}
    </article>
  );
}

export function ToolCallRow({
  tool,
  activeRunId,
}: {
  readonly tool: ToolCall;
  readonly activeRunId?: string;
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
  readonly activeRunId?: string;
}) {
  const [open, setOpen] = useState(false);
  const rowsId = useId();
  const summary = summarizeTools(group.items);
  const failures = group.items.filter((tool) => toolOutcome(tool, activeRunId) === 'failed').length;
  return (
    <div className="tool-calls" data-group="">
      <button
        type="button"
        className="tool-calls__toggle"
        aria-expanded={open}
        aria-controls={open ? rowsId : undefined}
        onClick={() => setOpen(!open)}
      >
        <span className="tool-calls__chevron" aria-hidden="true">
          {open ? <IconChevronDownOutline14 /> : <IconChevronRightOutline14 />}
        </span>
        <span className="tool-calls__count">{summary.label}</span>
        {summary.countsUnavailable ? <span>· Counts unavailable</span> : null}
        {summary.added === undefined ? null : (
          <>
            {' '}
            <span className="tool-call__added">+{summary.added}</span>{' '}
            <span className="tool-call__removed">−{summary.removed}</span>
          </>
        )}
        {summary.duration ? (
          <>
            {' '}
            <span className="tool-call__duration" title={summary.durationTitle}>
              {summary.duration}
            </span>
          </>
        ) : null}
        {failures ? (
          <>
            {' '}
            <span className="tool-calls__failed">· {failures} failed</span>
          </>
        ) : null}
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
