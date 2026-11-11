/** English message-footer labels, independent of locale and display timezone. */
export function relativeMessageTime(at: string | undefined, now: number): string | undefined {
  const timestamp = Date.parse(at ?? '');
  if (!Number.isFinite(timestamp)) return undefined;
  const elapsed = Math.max(0, now - timestamp);
  if (elapsed < 60_000) return 'just now';
  const [count, unit] =
    elapsed < 3_600_000
      ? [Math.floor(elapsed / 60_000), 'minute']
      : elapsed < 86_400_000
        ? [Math.floor(elapsed / 3_600_000), 'hour']
        : [Math.floor(elapsed / 86_400_000), 'day'];
  return `${count} ${unit}${count === 1 ? '' : 's'} ago`;
}
export function relativeMessageDelay(at: string, now: number): number {
  const elapsed = Math.max(0, now - Date.parse(at));
  const unit = elapsed < 3_600_000 ? 60_000 : elapsed < 86_400_000 ? 3_600_000 : 86_400_000;
  return Math.max(1, unit - (elapsed % unit));
}
