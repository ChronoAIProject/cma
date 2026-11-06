export function elapsedTime(milliseconds: number): string {
  const seconds = Math.max(0, Math.floor(milliseconds / 1000));
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);
  if (days) return `${days}d ${hours % 24}h`;
  if (hours) return `${hours}h ${minutes % 60}m`;
  return minutes ? `${minutes}m ${seconds % 60}s` : `${seconds}s`;
}

export function toolDuration(start?: string, end?: string): number | undefined {
  const duration = Date.parse(end ?? '') - Date.parse(start ?? '');
  return Number.isFinite(duration) && duration >= 0 ? duration : undefined;
}

export function durationLabel(milliseconds: number): string {
  return `${(milliseconds / 1000).toFixed(1)}s`;
}
