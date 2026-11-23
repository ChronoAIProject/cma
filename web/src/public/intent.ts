import type { CreateLaunchRequest, LaunchRepeatChoice } from '@cma/client';
import { routePath, type AppRoute } from '../routing/routes';

const KEY = 'cma:public-intent';
const LIFETIME = 15 * 60 * 1000;
export interface BrowserIntent {
  readonly id: string;
  readonly key: string;
  readonly path: string;
  readonly publicId: string;
  readonly expiresAt: number;
  readonly body: CreateLaunchRequest;
  readonly submitted: boolean;
  readonly owner?: string;
  readonly credentialRecovery?: 'refused' | 'retry';
}

export class IntentConflictError extends Error {
  readonly path?: string;

  constructor(saved?: BrowserIntent) {
    super(
      saved === undefined
        ? 'This tab has a saved request that could not be read safely. No new request has been sent. Contact your CMA administrator.'
        : 'An earlier request is still unresolved in this tab. Return to it with the same CMA account before starting another request.',
    );
    this.path = saved?.path;
  }
}

export function writeIntent(intent: BrowserIntent): void {
  const stored = sessionStorage.getItem(KEY);
  let raw: unknown;
  try {
    raw = JSON.parse(stored ?? 'null');
  } catch {
    throw new IntentConflictError();
  }
  const saved = parseIntent(raw);
  if (raw !== null && saved === undefined) throw new IntentConflictError();
  if (
    saved?.submitted &&
    (saved.id !== intent.id ||
      saved.owner !== intent.owner ||
      saved.path !== intent.path ||
      saved.publicId !== intent.publicId ||
      saved.expiresAt !== intent.expiresAt ||
      saved.body.start_if_ready !== intent.body.start_if_ready ||
      saved.body.placement_id !== intent.body.placement_id ||
      saved.body.referrer_origin !== intent.body.referrer_origin ||
      saved.body.choice !== intent.body.choice ||
      (saved.key !== intent.key &&
        !(saved.credentialRecovery === 'refused' && intent.credentialRecovery === 'retry')))
  )
    throw new IntentConflictError(saved);
  const submitted =
    intent.submitted || (saved?.submitted === true && intent.credentialRecovery !== 'refused');
  sessionStorage.setItem(KEY, JSON.stringify({ ...intent, submitted }));
}

export function hasCurrentConsent(intent: BrowserIntent): boolean {
  const now = Date.now();
  return intent.expiresAt > now && intent.expiresAt <= now + LIFETIME;
}

export function createIntent(
  route: Extract<AppRoute, { kind: 'public_trigger' }>,
  choice?: LaunchRepeatChoice,
): BrowserIntent {
  const id = crypto.randomUUID();
  let origin: string | undefined;
  try {
    origin = document.referrer === '' ? undefined : new URL(document.referrer).origin;
  } catch {
    origin = undefined;
  }
  const intent: BrowserIntent = {
    id,
    key: crypto.randomUUID(),
    path: routePath(route),
    publicId: route.publicId,
    expiresAt: Date.now() + LIFETIME,
    submitted: false,
    body: {
      placement_id: route.placementId,
      referrer_origin: origin === 'null' ? undefined : origin,
      start_if_ready: true,
      choice,
    },
  };
  writeIntent(intent);
  return intent;
}

export function readIntent(path: string): BrowserIntent | undefined {
  try {
    const intent = parseIntent(JSON.parse(sessionStorage.getItem(KEY) ?? 'null'));
    return intent?.path === path && (intent.submitted || hasCurrentConsent(intent))
      ? intent
      : undefined;
  } catch {
    return undefined;
  }
}

function parseIntent(raw: unknown): BrowserIntent | undefined {
  try {
    if (typeof raw !== 'object' || raw === null) return undefined;
    const intent = raw as Partial<BrowserIntent>;
    if (
      typeof intent.path !== 'string' ||
      typeof intent.id !== 'string' ||
      typeof intent.key !== 'string' ||
      typeof intent.publicId !== 'string' ||
      typeof intent.expiresAt !== 'number' ||
      !Number.isFinite(intent.expiresAt) ||
      typeof intent.submitted !== 'boolean' ||
      intent.body?.start_if_ready !== true
    )
      return undefined;
    if (
      (intent.owner !== undefined && typeof intent.owner !== 'string') ||
      (intent.submitted && typeof intent.owner !== 'string') ||
      (intent.credentialRecovery !== undefined &&
        (typeof intent.owner !== 'string' ||
          (intent.credentialRecovery !== 'refused' && intent.credentialRecovery !== 'retry'))) ||
      (intent.body.placement_id !== undefined && typeof intent.body.placement_id !== 'string') ||
      (intent.body.choice !== undefined &&
        intent.body.choice !== 'resume' &&
        intent.body.choice !== 'new')
    )
      return undefined;
    if (intent.body.referrer_origin !== undefined) {
      if (typeof intent.body.referrer_origin !== 'string') return undefined;
      const origin = new URL(intent.body.referrer_origin);
      if (
        !['https:', 'http:'].includes(origin.protocol) ||
        origin.origin !== intent.body.referrer_origin
      )
        return undefined;
    }
    const canonical = routePath({
      kind: 'public_trigger',
      publicId: intent.publicId,
      placementId: intent.body.placement_id,
    });
    if (canonical !== intent.path) return undefined;
    return {
      id: intent.id,
      key: intent.key,
      path: intent.path,
      publicId: intent.publicId,
      expiresAt: intent.expiresAt,
      submitted: intent.submitted,
      owner: intent.owner,
      credentialRecovery: intent.credentialRecovery,
      body: {
        start_if_ready: true,
        placement_id: intent.body.placement_id,
        referrer_origin: intent.body.referrer_origin,
        choice: intent.body.choice,
      },
    };
  } catch {
    return undefined;
  }
}

export function forgetIntent(id: string): void {
  try {
    const raw: unknown = JSON.parse(sessionStorage.getItem(KEY) ?? 'null');
    if (typeof raw === 'object' && raw !== null && 'id' in raw && raw.id === id)
      sessionStorage.removeItem(KEY);
  } catch {
    /* Storage failure must not hide recorded progress. */
  }
}
