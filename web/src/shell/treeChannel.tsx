import type { ChannelFacts } from '../resources/useChannelFacts';
import { channelLabel, channelFilters, type ChannelFilter } from '../resources/channelLabels';
import type { NavigationSandbox } from './navigation';
import { CreationSummary, type CreationFacts } from './treeCreation';

export function TreeFactsSummary({
  ids,
  facts,
  channelFacts,
}: {
  readonly ids: readonly string[];
  readonly facts: CreationFacts;
  readonly channelFacts: ChannelFacts;
}) {
  return (
    <>
      <CreationSummary ids={ids} facts={facts} />
      <ChannelSummary ids={ids} facts={channelFacts} />
    </>
  );
}

export function treeAgentIds(items: readonly NavigationSandbox[]): string[] {
  return items.flatMap(({ sandbox, workspaces }) =>
    sandbox.status === 'retired'
      ? []
      : workspaces
          .filter((workspace) => workspace.status !== 'removed')
          .flatMap((workspace) =>
            workspace.agents.filter((agent) => agent.status !== 'deleted').map((agent) => agent.id),
          ),
  );
}
export function ChannelBadge({ id, facts }: { readonly id: string; readonly facts: ChannelFacts }) {
  const value = facts.get(id);
  return value === undefined || !value.has_history ? null : (
    <span className="tree-row__origin" title={channelLabel(value)}>
      {value.active ? 'Channel linked' : 'Channel history'}
    </span>
  );
}
export function ChannelSummary({
  ids,
  facts,
}: {
  readonly ids: readonly string[];
  readonly facts: ChannelFacts;
}) {
  if (ids.length === 0 || ids.some((id) => facts.get(id) === undefined)) return null;
  const active = ids.filter((id) => facts.get(id)?.active).length;
  const historical = ids.filter((id) => facts.get(id)?.has_history).length;
  if (historical === 0) return null;
  const label = `${active} channel linked, ${historical} with channel history`;
  return (
    <span className="tree-row__origin" title={label}>
      {label}
    </span>
  );
}
export function filterTreeChannel(
  items: readonly NavigationSandbox[],
  facts: ChannelFacts,
  filter: ChannelFilter,
): readonly NavigationSandbox[] {
  if (filter === 'all') return items;
  return items.flatMap((item) => {
    const workspaces = item.workspaces
      .filter((workspace) => workspace.status !== 'removed')
      .flatMap((workspace) => {
        const agents = workspace.agents.filter((agent) => {
          const value = facts.get(agent.id);
          if (agent.status === 'deleted' || value === undefined) return false;
          return filter === 'active'
            ? value.active
            : filter === 'history'
              ? value.has_history
              : filter === 'detached'
                ? !value.active && value.has_history
                : !value.has_history;
        });
        return agents.length === 0 ? [] : [{ ...workspace, agents }];
      });
    return workspaces.length === 0 ? [] : [{ ...item, workspaces }];
  });
}
export function TreeChannelFilter({
  value,
  onChange,
  ids,
  facts,
}: {
  readonly value: ChannelFilter;
  readonly onChange: (value: ChannelFilter) => void;
  readonly ids: readonly string[];
  readonly facts: ChannelFacts;
}) {
  const incomplete = ids.some((id) => facts.get(id) === undefined);
  return (
    <div className="resource-tree__filter">
      <label>
        Agent channel activity
        <select
          aria-label="Agent channel activity"
          value={value}
          onChange={(event) => onChange(event.target.value as ChannelFilter)}
        >
          {channelFilters.map((filter) => (
            <option key={filter.value} value={filter.value}>
              {filter.label}
            </option>
          ))}
        </select>
      </label>
      {value === 'all' ? null : (
        <p role="status">
          {incomplete ? 'Some channel activity is loading or unavailable. ' : ''}Filtering Agents in
          this tree.
        </p>
      )}
    </div>
  );
}
