/** An authenticated account or its local sign-in epoch no longer owns this view. */
export class IdentityChangedError extends Error {
  constructor(options?: ErrorOptions) {
    super('Your sign-in changed. Open the Agent again to continue.', options);
    this.name = 'IdentityChangedError';
  }
}

/** SDK callbacks preserve their original failure as a protocol error cause. */
export function isIdentityChanged(error: unknown): boolean {
  const seen = new Set<unknown>();
  while (error instanceof Error && !seen.has(error)) {
    if (error instanceof IdentityChangedError) return true;
    seen.add(error);
    error = error.cause;
  }
  return false;
}
