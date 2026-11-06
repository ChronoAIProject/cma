import { elapsedTime } from '../agent/workspaceTime';
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
export function taskColor(id: string) {
  let hash = 0;
  for (const character of id) hash = (hash * 31 + character.codePointAt(0)!) >>> 0;
  return [
    'var(--dsw-alias-workspace-task-blue)',
    'var(--dsw-alias-workspace-task-purple)',
    'var(--dsw-alias-workspace-accent)',
  ][hash % 3]!;
}

export function runningTaskTime(at: string | undefined, now: Date) {
  const start = Date.parse(at ?? '');
  if (!Number.isFinite(start)) return '';
  const minutes = Math.floor(Math.max(0, now.getTime() - start) / 60_000);
  return minutes === 0 ? '<1m' : minutes < 60 ? `${minutes}m` : elapsedTime(now.getTime() - start);
}
export function nextTaskTick(starts: readonly number[], now: number) {
  const date = new Date(now);
  const midnight = new Date(date.getFullYear(), date.getMonth(), date.getDate() + 1).getTime();
  const next = starts
    .filter(Number.isFinite)
    .map((start) => start + (Math.max(0, Math.floor((now - start) / 60_000)) + 1) * 60_000);
  return Math.max(1, Math.min(midnight, ...next) - now);
}
