import { useAccessSummaries } from './useAccessSummaries';
import { useCallback, useMemo, useState } from 'react';
import { type AccessibleSandboxSummary } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { Button } from '../ui/Button';
import { useAuthorizedRead } from './useAuthorizedRead';
import { IconChevronDownOutline14, IconChevronRightOutline14 } from '../ui/icons';
import { scopeValue } from './scope';
import { SandboxBranch, type TreeExpansion } from './HierarchyTree';
import { keyboardNavigation } from '../shell/treePresentation';
import '../shell/Tree.css';

interface Props {
  readonly client: ResourceViewsClient;
  readonly route?: AppRoute;
  readonly organization?: string;
  readonly statusFilter?: string;
  readonly shared?: boolean;
  readonly onNavigate: (route: AppRoute) => void;
}

export function SandboxTree(props: Props) {
  return <SandboxTreePage key={scopeValue(props.organization, props.shared)} {...props} />;
}

function SandboxTreePage({
  client,
  organization,
  shared,
  onNavigate,
  route = { kind: 'home' },
  statusFilter = 'all',
}: Props) {
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const toggle = useCallback(
    (id: string) =>
      setCollapsed((previous) => {
        const next = new Set(previous);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        return next;
      }),
    [],
  );
  const collapse = useCallback(
    (id: string) => setCollapsed((previous) => new Set([...previous, id])),
    [],
  );
  const expansion = { collapsed, toggle, collapse };
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      const page = await client.sandboxes.listAccessible({
        organization,
        shared,
        cursor,
        limit: 50,
        signal,
      });
      if (
        page.next !== undefined &&
        (page.next === '' || (cursor !== undefined && page.next <= cursor))
      )
        throw new Error('Sandbox pagination did not advance. Refresh this scope.');
      let previous = cursor;
      for (const row of page.items) {
        if (previous !== undefined && row.id <= previous)
          throw new Error('Sandbox inventory order changed. Refresh this scope.');
        previous = row.id;
      }
      return { value: page };
    },
    [client, organization, shared, cursor],
  );
  const current = useAuthorizedRead(read);
  const accessSummaries = useAccessSummaries(
    client,
    'sandbox',
    current.state.status === 'ready' ? current.state.value.items.map((row) => row.id) : [],
    scopeValue(organization, shared),
  );
  return (
    <div className="scoped-navigation__tree resource-tree">
      <Button size="sm" onClick={current.reload}>
        {shared === true ? 'Refresh shared Sandboxes' : 'Refresh organization'}
      </Button>
      {current.state.status === 'loading' ? (
        <p role="status">
          {shared === true ? 'Loading shared Sandboxes' : 'Loading organization Sandboxes'}
        </p>
      ) : current.state.status === 'error' ? (
        <p role="alert">{current.state.message}</p>
      ) : (
        <>
          {current.state.value.items.length === 0 ? (
            <p>
              {shared === true
                ? 'No shared Sandboxes on this page. Individually shared Agents appear in Agents view.'
                : 'No authorized Sandboxes on this page. Agent-only grants appear in the Agent inventory.'}
            </p>
          ) : (
            <div>
              {['Live', 'Paused'].map((label) => (
                <SandboxGroup
                  key={label}
                  label={label}
                  items={
                    current.state.status === 'ready'
                      ? current.state.value.items.filter(
                          (sandbox) =>
                            sandbox.status !== 'retired' &&
                            (statusFilter === 'all' || sandbox.status === statusFilter) &&
                            (sandbox.status === 'paused') === (label === 'Paused'),
                        )
                      : []
                  }
                  client={client}
                  expansion={expansion}
                  route={route}
                  onNavigate={onNavigate}
                  accessSummaries={accessSummaries}
                  onAccessChanged={current.reload}
                />
              ))}
            </div>
          )}
          <div className="scoped-navigation__pages">
            <Button
              size="sm"
              disabled={cursors.length === 1}
              onClick={() => setCursors((values) => values.slice(0, -1))}
            >
              Previous Sandboxes
            </Button>
            <Button
              size="sm"
              disabled={current.state.value.next === undefined}
              onClick={() => {
                if (current.state.status === 'ready') {
                  const next = current.state.value.next;
                  if (next !== undefined) setCursors((values) => [...values, next]);
                }
              }}
            >
              Next Sandboxes
            </Button>
          </div>
        </>
      )}
    </div>
  );
}

function SandboxGroup({
  label,
  expansion,
  items,
  client,
  route,
  accessSummaries,
  onNavigate,
  onAccessChanged,
}: {
  readonly label: string;
  readonly expansion: TreeExpansion;
  readonly items: readonly AccessibleSandboxSummary[];
  readonly client: ResourceViewsClient;
  readonly route: AppRoute;
  readonly accessSummaries: ReadonlyMap<string, import('./accessSummary').AccessSummary>;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAccessChanged: () => void;
}) {
  const node = `group:${label}`;
  const open = !expansion.collapsed.has(node);
  return (
    <section className="resource-tree__section" aria-label={`${label} sandboxes`}>
      <button
        type="button"
        className="resource-tree__heading"
        aria-expanded={open}
        aria-label={`${open ? 'Collapse' : 'Expand'} ${label} sandboxes`}
        onClick={() => expansion.toggle(node)}
      >
        {open ? <IconChevronDownOutline14 /> : <IconChevronRightOutline14 />}
        <span>{label}</span>
        <span className="resource-tree__count">{items.length}</span>
      </button>
      {open ? (
        <div role="tree" aria-label={`${label} sandboxes`} onKeyDown={keyboardNavigation}>
          {items.map((sandbox) => (
            <SandboxBranch
              key={sandbox.id}
              client={client}
              sandbox={sandbox}
              expansion={expansion}
              route={route}
              accessSummary={accessSummaries.get(sandbox.id)}
              onNavigate={onNavigate}
              onAccessChanged={onAccessChanged}
            />
          ))}
          {items.length === 0 ? (
            <p className="resource-tree__empty">No {label.toLowerCase()} sandboxes</p>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}
