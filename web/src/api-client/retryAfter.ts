const MAX_RETRY_AFTER_MS = 2_147_483_647;

export function retryAfterMilliseconds(response: Response): number | undefined {
  const value = response.headers.get('retry-after')?.trim();
  if (value === undefined || value === '') return undefined;
  if (/^[0-9]+$/.test(value)) {
    const milliseconds = Number(value) * 1_000;
    return Number.isSafeInteger(milliseconds) &&
      milliseconds > 0 &&
      milliseconds <= MAX_RETRY_AFTER_MS
      ? milliseconds
      : undefined;
  }
  const milliseconds = Math.ceil(Date.parse(value) - Date.now());
  return Number.isFinite(milliseconds) && milliseconds > 0 && milliseconds <= MAX_RETRY_AFTER_MS
    ? milliseconds
    : undefined;
}
