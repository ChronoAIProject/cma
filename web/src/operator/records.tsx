import { useEffect, useState } from 'react';
import { Presence } from '../ui/Presence';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import {
  IconChevronLeftOutline14,
  IconChevronRightOutline14,
  IconRefreshOutline16,
} from '../ui/icons';
import { operatorFailure, type OperatorFailure } from './failure';

export interface RecordPage<T> {
  readonly items: readonly T[];
  readonly next?: string | null;
}

export function useRecords<T>(
  load: (cursor: string | undefined, signal: AbortSignal) => Promise<RecordPage<T>>,
) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const [reload, setReload] = useState(0);
  const [state, setState] = useState<{
    page?: RecordPage<T>;
    error?: OperatorFailure;
    loading: boolean;
  }>({ loading: true });
  const cursor = cursors.at(-1);
  useEffect(() => {
    const controller = new AbortController();
    load(cursor, controller.signal).then(
      (page) => {
        if (!controller.signal.aborted) setState({ page, loading: false });
      },
      (error: unknown) => {
        if (!controller.signal.aborted)
          setState((previous) => ({ ...previous, error: operatorFailure(error), loading: false }));
      },
    );
    return () => controller.abort();
  }, [load, cursor, reload]);
  const refresh = () => {
    setState((previous) => ({ ...previous, error: undefined, loading: true }));
    setReload((value) => value + 1);
  };
  return {
    ...state,
    refresh,
    previous:
      cursors.length > 1
        ? () => {
            setState({ loading: true });
            setCursors((value) => value.slice(0, -1));
          }
        : undefined,
    next:
      state.page?.next && !cursors.includes(state.page.next)
        ? () => {
            setState({ loading: true });
            setCursors((value) => [...value, state.page?.next ?? undefined]);
          }
        : undefined,
  };
}

export function FailureNotice({ failure }: { readonly failure?: OperatorFailure }) {
  return (
    <Presence>
      {failure === undefined ? null : (
        <Alert tone="error" title={failure.message}>
          {failure.field === undefined &&
          failure.code === undefined &&
          failure.requestId === undefined ? undefined : (
            <>
              {failure.field === undefined ? null : (
                <p>
                  Field: <code>{failure.field}</code>
                </p>
              )}
              {failure.code === undefined ? null : <code>{failure.code}</code>}
              {failure.requestId === undefined ? null : (
                <p>
                  Request <code>{failure.requestId}</code>
                </p>
              )}
            </>
          )}
        </Alert>
      )}
    </Presence>
  );
}

export function RecordControls({
  loading,
  refresh,
  previous,
  next,
}: {
  readonly loading: boolean;
  readonly refresh: () => void;
  readonly previous?: () => void;
  readonly next?: () => void;
}) {
  return (
    <div className="operator-actions">
      <Button
        size="sm"
        disabled={loading}
        aria-label="Refresh records"
        title="Refresh records"
        icon={<IconRefreshOutline16 />}
        onClick={refresh}
      />
      <Button
        size="sm"
        disabled={loading || previous === undefined}
        aria-label="Previous page"
        title="Previous page"
        icon={<IconChevronLeftOutline14 />}
        onClick={previous}
      />
      <Button
        size="sm"
        disabled={loading || next === undefined}
        aria-label="Next page"
        title="Next page"
        icon={<IconChevronRightOutline14 />}
        onClick={next}
      />
    </div>
  );
}

export function Timestamp({ value }: { readonly value: string }) {
  const date = new Date(value);
  return (
    <time dateTime={value}>
      {Number.isNaN(date.valueOf())
        ? value
        : date.toLocaleString('en-SG', {
            timeZone: 'Asia/Singapore',
            dateStyle: 'medium',
            timeStyle: 'short',
          })}
    </time>
  );
}

export function stateWord(value: string): string {
  return value.replaceAll('_', ' ');
}
