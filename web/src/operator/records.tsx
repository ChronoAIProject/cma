import { useEffect, useState, type ReactNode } from 'react';
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
  { retainOnError = true }: { readonly retainOnError?: boolean } = {},
) {
  const [pages, setPages] = useState<readonly { cursor: string | undefined; offset: number }[]>([
    { cursor: undefined, offset: 0 },
  ]);
  const [reload, setReload] = useState(0);
  const [state, setState] = useState<{
    page?: RecordPage<T>;
    error?: OperatorFailure;
    loading: boolean;
  }>({ loading: true });
  const { cursor, offset } = pages[pages.length - 1]!;
  const nextCursor = state.page?.next;
  const nextOffset = offset + (state.page?.items.length ?? 0);
  useEffect(() => {
    const controller = new AbortController();
    load(cursor, controller.signal).then(
      (page) => {
        if (!controller.signal.aborted) setState({ page, loading: false });
      },
      (error: unknown) => {
        if (!controller.signal.aborted)
          setState((previous) => ({
            ...(retainOnError ? previous : {}),
            error: operatorFailure(error),
            loading: false,
          }));
      },
    );
    return () => controller.abort();
  }, [load, cursor, reload, retainOnError]);
  const refresh = () => {
    setState((previous) => ({ ...previous, error: undefined, loading: true }));
    setReload((value) => value + 1);
  };
  return {
    ...state,
    refresh,
    pageNumber: pages.length,
    offset,
    previous:
      pages.length > 1
        ? () => {
            setState({ loading: true });
            setPages(pages.slice(0, -1));
          }
        : undefined,
    next:
      nextCursor && !pages.some(({ cursor }) => cursor === nextCursor)
        ? () => {
            setState({ loading: true });
            setPages([...pages, { cursor: nextCursor, offset: nextOffset }]);
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
  position,
}: {
  readonly loading: boolean;
  readonly refresh: () => void;
  readonly previous?: () => void;
  readonly next?: () => void;
  readonly position?: ReactNode;
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
      {position}
    </div>
  );
}

export function Timestamp({ value }: { readonly value: string }) {
  const date = new Date(value);
  return (
    <time dateTime={value} title={value}>
      {Number.isNaN(date.valueOf())
        ? value
        : date.toLocaleString('en-SG', {
            timeZone: 'Asia/Singapore',
            dateStyle: 'medium',
            timeStyle: 'medium',
          })}
    </time>
  );
}

export function stateWord(value: string): string {
  return value.replaceAll('_', ' ');
}
