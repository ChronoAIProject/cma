export const SIGN_OUT_NOTICE_KEY = 'cma.auth.sign-out';

export function isSignOutNotice(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(value)
  );
}

export function readSignOutNotice(): string | null {
  try {
    const notice = window.localStorage.getItem(SIGN_OUT_NOTICE_KEY);
    return isSignOutNotice(notice) ? notice : null;
  } catch {
    return null;
  }
}

export function openSignOutChannel(): BroadcastChannel | undefined {
  try {
    return new BroadcastChannel(SIGN_OUT_NOTICE_KEY);
  } catch {
    return undefined;
  }
}

export function publishSignOutNotice(): void {
  const notice = crypto.randomUUID();
  try {
    window.localStorage.setItem(SIGN_OUT_NOTICE_KEY, notice);
  } catch {
    // Live delivery is independent of persistent storage.
  }
  const channel = openSignOutChannel();
  channel?.postMessage(notice);
  channel?.close();
}
