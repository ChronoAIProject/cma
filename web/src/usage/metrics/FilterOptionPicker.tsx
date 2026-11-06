import { useCallback, useEffect, useState } from 'react';
import type { UsageFilterOptionsQuery } from '@chronoai/cma-sdk';
import { Button } from '../../ui/Button';
import { Menu } from '../../ui/Menu';
import { IconChevronDownOutline14 } from '../../ui/icons';
import { useUsageRead } from './useReport';
import type { MetricsClient } from './model';

/** One bounded page at a time, independent of every report/polling lifecycle. */
export function FilterOptionPicker({
  api,
  kind,
  owner,
  value,
  onChange,
  active,
}: {
  readonly api: MetricsClient;
  readonly kind: UsageFilterOptionsQuery['kind'];
  readonly owner?: string;
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly active: boolean;
}) {
  const label = kind === 'owner' ? 'Owner' : kind === 'sandbox' ? 'Sandbox' : 'Runtime version';
  const all = kind === 'owner' ? 'Everyone' : kind === 'sandbox' ? 'All Sandboxes' : 'Any runtime';
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const [query, setQuery] = useState({
    search: '',
    cursors: [undefined] as (string | undefined)[],
  });
  const [chosen, setChosen] = useState<{ value: string; label: string }>();
  const close = () => {
    setOpen(false);
    setSearch('');
    setQuery({ search: '', cursors: [undefined] });
  };
  useEffect(() => {
    if (search === query.search) return;
    const timer = setTimeout(() => setQuery({ search, cursors: [undefined] }), 250);
    return () => clearTimeout(timer);
  }, [search, query.search]);
  const cursor = query.cursors.at(-1);
  const load = useCallback(
    (signal: AbortSignal) =>
      api.options({
        kind,
        owner,
        search: query.search.trim(),
        cursor,
        limit: 50,
        signal,
      }),
    [api, kind, owner, query.search, cursor],
  );
  const page = useUsageRead(load, active && open && search === query.search);
  const resolve = useCallback(
    (signal: AbortSignal) =>
      api.options({
        kind,
        owner,
        selected: value,
        limit: 1,
        signal,
      }),
    [api, kind, owner, value],
  );
  const selected = useUsageRead(resolve, active && !!value && chosen?.value !== value);
  const current =
    chosen?.value === value ? chosen : selected.data?.items.find((item) => item.value === value);
  const currentLabel = !value
    ? all
    : (current?.label ??
      (selected.loading ? `Loading ${label.toLowerCase()}…` : `${label} unavailable`));
  const waiting = search !== query.search || page.loading;
  const failure = page.error ?? (value && !current ? selected.error : undefined);
  const options = waiting || page.error ? [] : (page.data?.items ?? []);
  const retry = () => {
    page.retry();
    selected.retry();
  };
  return (
    <div className="usage-metric-selector">
      <span className="usage-filter-label">{label}</span>
      <Menu
        open={open && active}
        label={`${label} choices`}
        placement="usage-filter-control"
        matchAnchorWidth
        selectedId={value}
        search={{
          label: `Search ${label.toLowerCase()} choices`,
          value: search,
          onChange: setSearch,
        }}
        items={[
          { id: '', label: all },
          ...options.map((option) => ({ id: option.value, label: option.label })),
        ]}
        onSelect={(next) => {
          setChosen(options.find((option) => option.value === next));
          onChange(next);
          close();
        }}
        onClose={close}
        anchor={
          <Button
            variant="outline"
            placement="usage-discovery-control"
            aria-label={`${label}: ${currentLabel}`}
            title={currentLabel}
            aria-haspopup="menu"
            aria-expanded={open && active}
            onClick={() => (open ? close() : setOpen(true))}
          >
            <span className="usage-discovery-value">{currentLabel}</span>
            <IconChevronDownOutline14 />
          </Button>
        }
        footer={
          <div className="usage-option-footer">
            {waiting && <p role="status">Loading {label.toLowerCase()} choices…</p>}
            {failure && (
              <div role="alert">
                <p>
                  {failure.denied
                    ? 'Filter choices are unavailable for your current access.'
                    : `Could not load ${label.toLowerCase()} choices. Try again.`}
                </p>
                <Button size="sm" onClick={retry}>
                  Retry choices
                </Button>
              </div>
            )}
            {!waiting && !failure && options.length === 0 && (
              <p role="status">No matching {label.toLowerCase()} choices.</p>
            )}
            {value && !current && !selected.loading && !selected.error && (
              <p role="status">
                The selected {label.toLowerCase()} is no longer available. Clear it or choose
                another.
              </p>
            )}
            <div className="usage-option-pages">
              <Button
                size="sm"
                disabled={waiting || query.cursors.length === 1}
                onClick={() =>
                  setQuery((previous) => ({ ...previous, cursors: previous.cursors.slice(0, -1) }))
                }
              >
                Previous choices
              </Button>
              <Button
                size="sm"
                disabled={waiting || !!page.error || !page.data?.next}
                onClick={() => {
                  const next = page.data?.next;
                  if (next && !query.cursors.includes(next))
                    setQuery((previous) => ({ ...previous, cursors: [...previous.cursors, next] }));
                }}
              >
                Next choices
              </Button>
            </div>
          </div>
        }
      />
    </div>
  );
}
