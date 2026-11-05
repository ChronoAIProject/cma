import type { TaskStatus } from './taskStatus';
export const TASK_GROUPS = ['Active', 'Today', 'Yesterday', 'Earlier'] as const;
export function taskGroup(
  status: TaskStatus,
  updatedAt: string,
  now: Date,
): (typeof TASK_GROUPS)[number] {
  if (
    status === 'working' ||
    status === 'not_started' ||
    status === 'starting' ||
    status === 'waiting_approval'
  )
    return 'Active';
  const date = new Date(updatedAt);
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1);
  if (date >= today) return 'Today';
  if (date >= yesterday) return 'Yesterday';
  return 'Earlier';
}
export function taskTime(updatedAt: string, now: Date): string {
  const date = new Date(updatedAt);
  if (!Number.isFinite(date.getTime())) return '—';
  const today =
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate();
  return new Intl.DateTimeFormat(
    today ? 'en-GB' : 'en-US',
    today
      ? { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }
      : { month: 'short', day: 'numeric' },
  ).format(date);
}
export function taskHue(id: string) {
  let hash = 0;
  for (const character of id) hash = (hash * 31 + character.codePointAt(0)!) >>> 0;
  return hash % 360;
}
