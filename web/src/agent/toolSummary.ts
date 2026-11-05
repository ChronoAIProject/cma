import type { ToolCall } from './model';
import { durationLabel, toolDuration } from './workspaceTime';
import { screenOperation } from './screenOperations';
import { reportedFileChanges } from './fileChanges';
export function toolCategory(tool: ToolCall): 'command' | 'read' | 'edit' | 'other' {
  if (tool.kind === 'execute') return 'command';
  if (tool.kind === 'read') return 'read';
  if (tool.kind === 'edit' || tool.kind === 'delete') return 'edit';
  return 'other';
}
export function diffCounts(text: string) {
  const lines = text.split('\n');
  return {
    added: lines.filter((line) => line.startsWith('+') && !line.startsWith('+++')).length,
    removed: lines.filter((line) => line.startsWith('-') && !line.startsWith('---')).length,
  };
}
export function summarizeTools(tools: readonly ToolCall[]): {
  label: string;
  added?: number;
  removed?: number;
  duration?: string;
} {
  if (!tools.length) return { label: '' };
  const screens = tools.map(screenOperation);
  if (screens.every((step) => step !== undefined)) {
    const app = screens.every((step) => step.application === 'Browser') ? 'Browser' : 'Desktop';
    const duration = toolDuration(tools[0]?.startedAt, tools.at(-1)?.finishedAt);
    return {
      label: `Checked it in ${app} · ${tools.length} step${tools.length === 1 ? '' : 's'}`,
      duration:
        duration === undefined
          ? undefined
          : duration % 1000 === 0
            ? `${duration / 1000}s`
            : durationLabel(duration),
    };
  }
  const count = (category: ReturnType<typeof toolCategory>) =>
    tools.filter((tool) => toolCategory(tool) === category);
  const commands = count('command'),
    reads = count('read'),
    edits = count('edit');
  const parts: string[] = [];
  if (commands.length)
    parts.push(`Ran ${commands.length} command${commands.length === 1 ? '' : 's'}`);
  if (reads.length) parts.push(`Read ${reads.length} file${reads.length === 1 ? '' : 's'}`);
  let added: number | undefined, removed: number | undefined;
  if (edits.length) {
    const changes = edits.map((tool) => ({ tool, ...reportedFileChanges(tool.output?.text) }));
    const names = [
      ...new Set(
        changes.flatMap(({ tool, files, total }) =>
          files.length && (!total || files.length === total.count)
            ? files.map((file) => file.path.split('/').pop()!)
            : total
              ? [`${total.count} files`]
              : [/^editing files$/i.test(tool.title) ? 'files' : tool.title],
        ),
      ),
    ];
    parts.push(`Edited ${names.join(', ')}`);
    const counts = changes.map(
      ({ tool, total }) =>
        total ?? (tool.output && !tool.output.truncated ? diffCounts(tool.output.text) : undefined),
    );
    if (
      counts.every((count) => count !== undefined) &&
      counts.some((count) => count.added || count.removed)
    ) {
      added = counts.reduce((sum, count) => sum + count.added, 0);
      removed = counts.reduce((sum, count) => sum + count.removed, 0);
    }
  }
  for (const tool of count('other')) parts.push(tool.title.trim() || 'Tool operation');
  const durations = tools.map((tool) => toolDuration(tool.startedAt, tool.finishedAt));
  return {
    label: parts.join(' · '),
    added,
    removed,
    duration: durations.every((value) => value !== undefined)
      ? durationLabel(durations.reduce((sum, value) => sum + value, 0))
      : undefined,
  };
}
export function toolGroupSummary(tools: readonly ToolCall[]): string {
  const summary = summarizeTools(tools);
  return (
    summary.label +
    (summary.added === undefined ? '' : ` +${summary.added} −${summary.removed}`) +
    (summary.duration ? `  ${summary.duration}` : '')
  );
}
