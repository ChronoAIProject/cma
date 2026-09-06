import { useEffect, useId, useState } from 'react';
import type { MarketplaceSummary, ProfileKind, ProfileReference } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { IconRefreshOutline16, IconSearchOutline16 } from '../ui/icons';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice } from './records';

export function DefaultProfilePicker({
  client,
  kind,
  value,
  disabled,
  error,
  onChange,
}: {
  readonly client: OperatorClient;
  readonly kind: ProfileKind;
  readonly value: ProfileReference;
  readonly disabled: boolean;
  readonly error?: string;
  readonly onChange: (value: ProfileReference) => void;
}) {
  const errorId = useId();
  const [search, setSearch] = useState('');
  const [query, setQuery] = useState('');
  const [cursor, setCursor] = useState<string>();
  const [reload, setReload] = useState(0);
  const [state, setState] = useState<{
    items: readonly MarketplaceSummary[];
    next?: string;
    loading: boolean;
    error?: OperatorFailure;
  }>({ items: [], loading: true });
  useEffect(() => {
    const controller = new AbortController();
    client
      .listPublicProfiles(kind, {
        q: query || undefined,
        cursor,
        limit: 50,
        signal: controller.signal,
      })
      .then(
        (page) => {
          if (!controller.signal.aborted)
            setState((previous) => ({
              items:
                cursor === undefined
                  ? page.items
                  : [
                      ...previous.items,
                      ...page.items.filter(
                        (item) =>
                          !previous.items.some(
                            (prior) =>
                              prior.id === item.id &&
                              prior.latest_revision === item.latest_revision,
                          ),
                      ),
                    ],
              next: page.next,
              loading: false,
            }));
        },
        (error: unknown) => {
          if (!controller.signal.aborted)
            setState((previous) => ({
              ...previous,
              error: operatorFailure(error),
              loading: false,
            }));
        },
      );
    return () => controller.abort();
  }, [client, kind, query, cursor, reload]);
  const selected = `${value.id}:${value.revision}`;
  const containsSelected = state.items.some(
    (item) => `${item.id}:${item.latest_revision}` === selected,
  );
  return (
    <div className="operator-default-picker">
      <label className="operator-field">
        <span>
          {kind[0]?.toUpperCase()}
          {kind.slice(1)} profile
        </span>
        <select
          aria-label={`Default ${kind} profile`}
          value={selected}
          disabled={disabled}
          aria-invalid={error !== undefined}
          aria-describedby={error === undefined ? undefined : errorId}
          onChange={(event) => {
            const option = state.items.find(
              (item) => `${item.id}:${item.latest_revision}` === event.target.value,
            );
            if (option !== undefined) onChange({ id: option.id, revision: option.latest_revision });
          }}
        >
          {containsSelected ? null : (
            <option value={selected}>
              {value.id} / revision {value.revision} (current)
            </option>
          )}
          {state.items.map((item) => (
            <option
              key={`${item.id}:${item.latest_revision}`}
              value={`${item.id}:${item.latest_revision}`}
            >
              {item.name} / revision {item.latest_revision} / {item.owner_display}
            </option>
          ))}
        </select>
      </label>
      {error === undefined ? null : (
        <p className="operator-failure" id={errorId}>
          {error}
        </p>
      )}
      <div className="operator-actions">
        <Input
          aria-label={`Search public ${kind} profiles`}
          value={search}
          disabled={disabled}
          maxLength={255}
          onChange={(event) => setSearch(event.target.value)}
        />
        <Button
          size="sm"
          disabled={disabled || state.loading}
          aria-label={`Search ${kind} profiles`}
          title={`Search ${kind} profiles`}
          icon={<IconSearchOutline16 />}
          onClick={() => {
            setState({ items: [], loading: true });
            setCursor(undefined);
            setQuery(search);
            setReload((value) => value + 1);
          }}
        />
        <Button
          size="sm"
          disabled={disabled || state.loading}
          aria-label={`Refresh ${kind} profiles`}
          title={`Refresh ${kind} profiles`}
          icon={<IconRefreshOutline16 />}
          onClick={() => {
            setState((value) => ({ ...value, loading: true, error: undefined }));
            setCursor(undefined);
            setReload((value) => value + 1);
          }}
        />
        {state.next === undefined ? null : (
          <Button
            size="sm"
            disabled={disabled || state.loading || cursor === state.next}
            onClick={() => {
              setState((value) => ({ ...value, loading: true }));
              setCursor(state.next);
            }}
          >
            More profiles
          </Button>
        )}
      </div>
      {state.loading ? <p role="status">Loading public {kind} profiles</p> : null}
      <FailureNotice failure={state.error} />
    </div>
  );
}
