import type { ChannelListItem } from './ChannelEditor';

export function duplicateChannelLabels(channels: readonly ChannelListItem[]) {
  const idsByName = new Map<string, Set<string>>();
  for (const { definition, record_id } of channels) {
    const ids = idsByName.get(definition.name) ?? new Set<string>();
    ids.add(record_id);
    idsByName.set(definition.name, ids);
  }

  const labels = new Map<string, string>();
  for (const [name, group] of idsByName) {
    const ids = [...group];
    if (ids.length < 2) continue;
    let length = 8;
    while (new Set(ids.map((id) => id.slice(0, length))).size < ids.length) length += 1;
    // Keep the reference visible when a long name is truncated in the sidebar.
    for (const id of ids) labels.set(id, `${id.slice(0, length)} · ${name}`);
  }
  return labels;
}
