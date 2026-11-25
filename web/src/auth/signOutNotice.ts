export const SIGN_OUT_NOTICE_KEY = 'cma.auth.sign-out';

export function readSignOutNotice(): string | null {
  try {
    return window.localStorage.getItem(SIGN_OUT_NOTICE_KEY);
  } catch {
    return null;
  }
}

export function publishSignOutNotice(): void {
  try {
    window.localStorage.setItem(SIGN_OUT_NOTICE_KEY, crypto.randomUUID());
  } catch {
    // Browser storage can be unavailable. The server has already revoked the session.
  }
}
