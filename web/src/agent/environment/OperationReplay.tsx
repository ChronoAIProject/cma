import { memo, useCallback, useEffect, useRef, useState } from 'react';
import { replayError } from './replayError';
import { Button } from '../../ui/Button';
import { OperationDetails } from './OperationDetails';
import type { ToolCall } from '../model';
import {
  readReplayWindow,
  advanceReplayWindow,
  type ReplayQuery,
  type ReplayReader,
  type ReplayWindow,
} from './replayIndex';

export const OperationReplay = memo(function OperationReplay({
  reader,
  agentId,
  scope,
  toolCalls,
  selection,
  onSelectionHandled,
}: {
  readonly reader: ReplayReader;
  readonly agentId: string;
  readonly scope: string;
  readonly toolCalls?: readonly ToolCall[];
  readonly selection?: { readonly turn: string };
  readonly onSelectionHandled?: () => void;
}) {
  const storageKey = `cma.operation-replay:${scope}:${agentId}`;
  const [selected, setSelected] = useState<string | undefined>(() => {
    try {
      return sessionStorage.getItem(storageKey) ?? undefined;
    } catch {
      return undefined;
    }
  });
  const selectedRef = useRef(selected);
  const [historical, setHistorical] = useState(selected !== undefined || selection !== undefined);
  const [live, setLive] = useState<ReplayWindow>();
  const liveRef = useRef<ReplayWindow | undefined>(undefined);
  const [history, setHistory] = useState<ReplayWindow>();
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [loading, setLoading] = useState(true);
  const request = useRef<AbortController | undefined>(undefined);
  const choose = useCallback(
    (cursor?: string) => {
      selectedRef.current = cursor;
      setSelected(cursor);
      setHistorical(cursor !== undefined);
      try {
        if (cursor) sessionStorage.setItem(storageKey, cursor);
        else sessionStorage.removeItem(storageKey);
      } catch {
        return;
      }
    },
    [storageKey],
  );
  const load = useCallback(
    async (query: ReplayQuery = {}, select: 'last' | 'first' | 'keep' | 'live' = 'keep') => {
      request.current?.abort();
      const controller = new AbortController();
      request.current = controller;
      setLoading(true);
      setError(undefined);
      setNotice(undefined);
      try {
        const page =
          select === 'keep' && !query.through && !query.after && !query.turn && liveRef.current
            ? await advanceReplayWindow(reader, agentId, controller.signal, liveRef.current)
            : await readReplayWindow(reader, agentId, controller.signal, query);
        if (controller.signal.aborted) return;
        if (select === 'first' && page.records.length === 0) {
          setNotice('No newer operations recorded.');
          return;
        }
        if (query.through || query.after || query.turn) {
          setHistory(page);
          if (select !== 'keep')
            choose((select === 'first' ? page.records[0] : page.records.at(-1))?.cursor);
          setHistorical(true);
          if (query.turn && page.records.length > 0) onSelectionHandled?.();
        } else {
          setLive(page);
          liveRef.current = page;
          if (select === 'live') {
            choose();
            setHistory(undefined);
            onSelectionHandled?.();
          }
        }
      } catch (cause) {
        if (controller.signal.aborted) return;
        setHistory(undefined);
        setLive(undefined);
        liveRef.current = undefined;
        setError(replayError(cause));
      } finally {
        if (!controller.signal.aborted) setLoading(false);
      }
    },
    [reader, agentId, choose, onSelectionHandled],
  );
  useEffect(() => {
    void load(
      selection
        ? { turn: selection.turn }
        : selectedRef.current
          ? { through: selectedRef.current }
          : {},
      selection ? 'last' : 'keep',
    );
    return () => request.current?.abort();
  }, [load, selection]);
  const observed = useRef(toolCalls);
  useEffect(() => {
    if (observed.current === toolCalls) return;
    observed.current = toolCalls;
    if (!selectedRef.current) void load();
  }, [toolCalls, load]);
  const page = historical ? (history ?? live) : live;
  const record = historical
    ? page?.records.find((item) => item.cursor === selected)
    : page?.records.at(-1);
  const position = record ? page!.records.indexOf(record) : -1;
  const previous = () => {
    if (position > 0) {
      setHistory(page);
      choose(page!.records[position - 1]!.cursor);
    } else if (page?.older) void load({ through: page.older }, 'last');
  };
  const next = () => {
    if (page && position + 1 < page.records.length) choose(page.records[position + 1]!.cursor);
    else if (record) void load({ after: record.cursor }, 'first');
  };
  const returnLive = () => {
    void load({}, 'live');
  };
  return (
    <section className="operation-replay" aria-label="Operation record replay">
      <h3>Operation record replay</h3>
      <p>{historical ? 'History · Read only' : 'Live · Recorded operations'}</p>
      <nav
        className="operation-replay__navigation"
        aria-label="Replay navigation"
        onKeyDown={(event) => {
          if (loading || event.target instanceof HTMLInputElement) return;
          if (event.key === 'ArrowLeft') {
            event.preventDefault();
            previous();
          }
          if (event.key === 'ArrowRight' && selected) {
            event.preventDefault();
            next();
          }
          if (event.key === 'End') {
            event.preventDefault();
            returnLive();
          }
        }}
      >
        <Button size="sm" disabled={loading || (position <= 0 && !page?.older)} onClick={previous}>
          Previous operation
        </Button>
        <Button size="sm" disabled={loading || !selected || !record} onClick={next}>
          Next operation
        </Button>
        <Button size="sm" disabled={loading} onClick={returnLive}>
          Return Live
        </Button>
      </nav>
      {page && record ? (
        <label className="operation-replay__position">
          <span>
            Recorded operation {position + 1} of {page.records.length} in this window
          </span>
          <input
            type="range"
            aria-label="Recorded operation in this window"
            min={1}
            max={page.records.length}
            value={position + 1}
            disabled={loading || page.records.length < 2}
            aria-valuetext={`Operation ${position + 1} of ${page.records.length} in this window`}
            onChange={(event) => {
              const selectedRecord = page.records[Number(event.target.value) - 1];
              if (!selectedRecord) return;
              setHistory(page);
              choose(selectedRecord.cursor);
            }}
          />
        </label>
      ) : null}
      {loading ? <p role="status">Reading authorized operation history…</p> : null}
      {error ? (
        <>
          <p role="alert">{error}</p>
          <Button
            size="sm"
            disabled={loading}
            onClick={() => void load(selected ? { through: selected } : {}, 'keep')}
          >
            Retry history read
          </Button>
        </>
      ) : null}
      {notice ? <p role="status">{notice}</p> : null}
      {page?.gap ? (
        <p role="status">History gap: some operations and plan snapshots were not recorded.</p>
      ) : null}
      {record ? (
        <article aria-label={`Recorded operation: ${record.title}`}>
          <p role="status">
            Record {position + 1} of {page!.records.length} in this window
          </p>
          <OperationDetails
            key={record.cursor}
            number={position + 1}
            title={record.title}
            status={record.status}
            output={record.output}
          >
            <p>Turn {record.turn}</p>
            <p>Cursor {record.cursor}</p>
            <p>
              Recorded time: <time dateTime={record.at}>{record.at}</time>
            </p>
          </OperationDetails>
          {record.plan ? (
            <section aria-label="Plan at this operation">
              <h4>Plan at this operation</h4>
              <ol>
                {record.plan.map((entry, index) => (
                  <li key={index}>
                    {entry.title}: {entry.status}
                  </li>
                ))}
              </ol>
              {record.plan.length === 0 ? <p>Plan cleared</p> : null}
            </section>
          ) : (
            <p>Plan not recorded at this operation</p>
          )}
          <p>Browser frames and file snapshots not recorded.</p>
        </article>
      ) : !loading && !error ? (
        <p>No command operations recorded for this turn.</p>
      ) : null}
      {historical ? (
        <p>
          History controls unavailable: commands, approvals, feedback, suggestions and computer
          control.
        </p>
      ) : null}
    </section>
  );
});
