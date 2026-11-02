export function uptime(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return `${days > 0 ? `${String(days)}d ` : ''}${String(hours)}h ${String(minutes)}m ${String(seconds % 60)}s`;
}
export function usageTime(seconds: number): string {
  return new Date(seconds * 1000).toLocaleString('en-SG', { timeZone: 'Asia/Singapore' });
}
