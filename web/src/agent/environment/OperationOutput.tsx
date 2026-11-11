import { memo, useEffect, useRef } from 'react';
import type { ToolCall } from '../model';
import { durationLabel, toolDuration } from '../workspaceTime';
import { useTerminalSelection, type AgentComputerObservation } from './observation';

export const OperationOutput = memo(function OperationOutput({
  toolCalls,
  activeRunId,
  active,
  observation,
  workingDirectory,
}: {
  readonly toolCalls?: readonly ToolCall[];
  readonly activeRunId?: string;
  readonly active: boolean;
  readonly observation?: AgentComputerObservation;
  readonly workingDirectory?: string;
}) {
  const root = useRef<HTMLDivElement>(null);
  const selected = useTerminalSelection(observation);
  const applied = useRef<typeof selected>(undefined);
  const selectedPresent = !!toolCalls?.some(
    (tool) => tool.kind === 'execute' && tool.id === selected?.id,
  );
  useEffect(() => {
    if (!active || !selected || !selectedPresent || !root.current || applied.current === selected)
      return;
    const block = [...root.current.querySelectorAll<HTMLElement>('[data-command-id]')].find(
      (element) => element.dataset.commandId === selected.id,
    );
    const scrollport = root.current.closest('.agent-computer__page');
    if (!block || !(scrollport instanceof HTMLElement)) return;
    const terminal = root.current;
    const align = () => {
      terminal.style.paddingBottom = '0px';
      const portTop = scrollport.getBoundingClientRect().top;
      const target = scrollport.scrollTop + block.getBoundingClientRect().top - portTop - 16;
      const contentEnd = terminal.getBoundingClientRect().bottom - portTop + scrollport.scrollTop;
      terminal.style.paddingBottom = `${Math.ceil(Math.max(0, target + scrollport.clientHeight - contentEnd))}px`;
      scrollport.scrollTop = target;
    };
    align();
    let width = scrollport.clientWidth,
      height = scrollport.clientHeight;
    const observer =
      typeof ResizeObserver === 'undefined'
        ? undefined
        : new ResizeObserver(() => {
            if (width === scrollport.clientWidth && height === scrollport.clientHeight) return;
            width = scrollport.clientWidth;
            height = scrollport.clientHeight;
            align();
          });
    observer?.observe(scrollport);
    block.focus({ preventScroll: true });
    applied.current = selected;
    for (const previous of root.current.querySelectorAll('[data-highlighted]'))
      previous.removeAttribute('data-highlighted');
    void block.offsetWidth;
    const frame = requestAnimationFrame(() => block.setAttribute('data-highlighted', 'true'));
    const timer = window.setTimeout(() => {
      block.removeAttribute('data-highlighted');
      observer?.disconnect();
    }, 1800);
    return () => {
      observer?.disconnect();
      cancelAnimationFrame(frame);
      clearTimeout(timer);
      block.removeAttribute('data-highlighted');
    };
  }, [active, selected, selectedPresent]);
  if (!toolCalls) return active ? <p role="status">Loading recorded operations…</p> : null;
  const operations = toolCalls.filter((tool) => tool.kind === 'execute');
  if (!operations.length) return <p>No command operations reported for this Agent.</p>;
  return (
    <div className="workspace-terminal" ref={root} hidden={!active} inert={!active || undefined}>
      <header className="workspace-terminal__header">
        <strong>bash</strong>
        <span>{workingDirectory}</span>
        {activeRunId ? <span>● Live</span> : null}
      </header>
      {operations.map((tool) => {
        const duration = toolDuration(tool.startedAt, tool.finishedAt);
        return (
          <article
            key={tool.id}
            className="workspace-terminal__block"
            data-command-id={tool.id}
            tabIndex={-1}
            aria-label={`Operation: ${tool.title}`}
          >
            <header>
              <code aria-label="Recorded command">$ {tool.detail ?? tool.title}</code>
              {duration === undefined ? null : <time>{durationLabel(duration)}</time>}
            </header>
            {tool.output ? (
              <>
                <pre aria-label="Recorded output">{tool.output.text}</pre>
                {tool.output.truncated ? <p>Output truncated</p> : null}
              </>
            ) : (
              <p>Output not reported</p>
            )}
          </article>
        );
      })}
      <p className="workspace-terminal__end">
        {activeRunId ? <span aria-label="Terminal cursor">▌</span> : '— agent paused —'}
      </p>
    </div>
  );
});
