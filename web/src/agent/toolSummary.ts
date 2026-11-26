import type { ToolCall } from './model';
import { toolGroupTimingLabel } from './toolTiming';
import { screenOperation } from './screenOperations';
import { reportedFileChanges } from './fileChanges';
export function toolCategory(
  tool: ToolCall,
): 'command' | 'read' | 'edit' | 'search' | 'list' | 'other' {
  if (tool.kind === 'execute') return 'command';
  if (tool.kind === 'search') return 'search';
  if (tool.kind === 'read')
    return /^list(?:ed)? (?:files|folders|directories|directory)\b/i.test(tool.title)
      ? 'list'
      : 'read';
  if (tool.kind === 'edit' || tool.kind === 'delete') return 'edit';
  return 'other';
}
export function diffCounts(text: string) {
  if (!/^@@ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@/m.test(text)) return undefined;
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
  durationTitle?: string;
  countsUnavailable?: boolean;
} {
  if (!tools.length) return { label: '' };
  const screens = tools.map(screenOperation);
  if (screens.every((step) => step !== undefined)) {
    const app = screens.every((step) => step.application === 'Browser') ? 'Browser' : 'Desktop';
    return {
      label: `Checked it in ${app} · ${tools.length} step${tools.length === 1 ? '' : 's'}`,
      duration: toolGroupTimingLabel(tools, true),
      durationTitle: 'Recorded interval from the first step to the last step',
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
  const searches = count('search'),
    listings = count('list');
  if (searches.length)
    parts.push(`Searched ${searches.length} time${searches.length === 1 ? '' : 's'}`);
  if (listings.length)
    parts.push(`Listed ${listings.length} folder${listings.length === 1 ? '' : 's'}`);
  let added: number | undefined, removed: number | undefined;
  let countsUnavailable = false;
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
    countsUnavailable = changes.some(
      ({ total }) => total !== undefined && total.added === undefined,
    );
    if (
      counts.every(
        (count): count is { added: number; removed: number } =>
          count?.added !== undefined && count.removed !== undefined,
      ) &&
      counts.some((count) => count.added || count.removed)
    ) {
      added = counts.reduce((sum, count) => sum + count.added, 0);
      removed = counts.reduce((sum, count) => sum + count.removed, 0);
    }
  }
  for (const tool of count('other')) parts.push(tool.title.trim() || 'Tool operation');
  return {
    label: parts.join(' · '),
    countsUnavailable,
    added,
    removed,
    duration: toolGroupTimingLabel(tools, false),
    durationTitle: 'Sum of recorded tool intervals',
  };
}
export function toolGroupSummary(tools: readonly ToolCall[]): string {
  const summary = summarizeTools(tools);
  return (
    summary.label +
    (summary.countsUnavailable ? ' · Counts unavailable' : '') +
    (summary.added === undefined ? '' : ` +${summary.added} −${summary.removed}`) +
    (summary.duration ? `  ${summary.duration}` : '')
  );
}

export function conciseToolError(tool: ToolCall): string {
  const fallback =
    tool.title === 'mcp.cma_access.describe_deliverable'
      ? 'Deliverable result unavailable (result_projection_unavailable). Retryable: unknown. Check the operation history and runtime readiness before verifying the same resource again.'
      : 'The operation failed.';
  const text = tool.output?.text || tool.detail || fallback;
  const lines = text.trim().split('\n').slice(-3);
  const exit = /(?:exit(?:ed)?(?: with)?(?: code|_code)?[:= ]+)(-?\d+)/i.exec(text);
  if (exit && !lines.some((line) => line.includes(exit[0]!)))
    lines[0] = `Exit code ${exit[1]} · ${lines[0]}`;
  return lines.map((line) => (line.length > 240 ? `${line.slice(0, 239)}…` : line)).join('\n');
}
