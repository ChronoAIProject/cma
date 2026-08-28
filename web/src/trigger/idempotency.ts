/** New browser-owned intent key. It is never persisted or reused across different commands. */
export function newIntentKey(): string {
  return `cma-web-${crypto.randomUUID()}`;
}
