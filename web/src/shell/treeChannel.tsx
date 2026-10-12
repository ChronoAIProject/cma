import type { ChannelFacts } from '../resources/useChannelFacts';
import { channelLabel } from '../resources/channelLabels';
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
