import type { ToolCall } from './model';
import { toolOutcome } from './toolStatus';
export interface FileChange {
  readonly path: string;
  readonly added: number;
  readonly removed: number;
}
export function reportedFileChanges(output: string | undefined) {
  const files: FileChange[] = [];
  let total: { count: number; added: number; removed: number } | undefined;
  for (const line of (output ?? '').split('\n')) {
    const aggregate = /^File changes: (\d+) \+(\d+) −(\d+)$/.exec(line);
    if (aggregate) {
      const [count, added, removed] = aggregate.slice(1).map(Number);
      if ([count, added, removed].every(Number.isSafeInteger))
        total = { count: count!, added: added!, removed: removed! };
    }
    const match = /^File: ("(?:[^"\\]|\\.)*") \+(\d+) −(\d+)$/.exec(line);
    if (!match) continue;
    try {
      const path: unknown = JSON.parse(match[1]!);
      const added = Number(match[2]),
        removed = Number(match[3]);
      if (
        typeof path === 'string' &&
        path &&
        Number.isSafeInteger(added) &&
        Number.isSafeInteger(removed)
      )
        files.push({ path, added, removed });
    } catch {
      /* Unrecognized output remains visible as ordinary recorded text. */
    }
  }
  return { files, total };
}
export function homeFilePath(path: string, directory?: string): string | undefined {
  const absolute = path.startsWith('/') ? path : directory ? `${directory}/${path}` : undefined;
  if (!absolute?.startsWith('/home/')) return undefined;
  const parts: string[] = [];
  for (const part of absolute.slice(6).split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') {
      if (!parts.length) return undefined;
      parts.pop();
    } else parts.push(part);
  }
  return parts.join('/');
}
/** Counts describe completed reported edits; they do not invent a Git baseline. */
export function changedFiles(
  tools: readonly ToolCall[] | undefined,
  directory?: string,
): ReadonlyMap<string, FileChange> {
  const result = new Map<string, FileChange>();
  for (const tool of tools ?? []) {
    if (tool.kind !== 'edit' || toolOutcome(tool, undefined) !== 'completed') continue;
    for (const change of reportedFileChanges(tool.output?.text).files) {
      const path = homeFilePath(change.path, directory);
      if (!path) continue;
      const previous = result.get(path);
      result.set(path, {
        path,
        added: (previous?.added ?? 0) + change.added,
        removed: (previous?.removed ?? 0) + change.removed,
      });
    }
  }
  return result;
}
