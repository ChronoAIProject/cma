import type { ToolCall } from './model';
import { isPendingTool } from './toolStatus';
import { durationLabel, toolDuration } from './workspaceTime';

const unavailable = 'Duration unavailable';
const hasFraction = (value?: string) =>
  /T\d{2}:\d{2}:\d{2}\.\d+(?:Z|[+-]\d{2}:\d{2})$/.test(value ?? '');
function recordedTiming(tool: ToolCall) {
  const milliseconds = toolDuration(tool.startedAt, tool.finishedAt);
  return milliseconds === undefined
    ? undefined
    : {
        milliseconds,
        wholeSeconds: !hasFraction(tool.startedAt) || !hasFraction(tool.finishedAt),
      };
}

export function toolTimingLabel(tool: ToolCall): string | undefined {
  if (isPendingTool(tool)) return undefined;
  const timing = recordedTiming(tool);
  return timing === undefined
    ? unavailable
    : durationLabel(timing.milliseconds, timing.wholeSeconds);
}

export function toolGroupTimingLabel(
  tools: readonly ToolCall[],
  screenInterval: boolean,
): string | undefined {
  if (!tools.length || tools.some(isPendingTool)) return undefined;
  const timings = tools.map(recordedTiming);
  if (!timings.every((timing) => timing !== undefined)) return unavailable;
  const milliseconds = screenInterval
    ? toolDuration(tools[0]?.startedAt, tools.at(-1)?.finishedAt)
    : timings.reduce((sum, timing) => sum + timing.milliseconds, 0);
  if (milliseconds === undefined) return unavailable;
  const wholeSeconds =
    timings.some((timing) => timing.wholeSeconds) || (screenInterval && milliseconds % 1000 === 0);
  return durationLabel(milliseconds, wholeSeconds);
}
