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
  readonly wireBody?: string;
  readonly recoveryRequired?: true;
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
  const credentialRetry =
    saved?.credentialRecovery === 'refused' &&
    intent.credentialRecovery === 'retry' &&
    saved.key !== intent.key;
  if (
    saved?.submitted &&
    (saved.id !== intent.id ||
      saved.owner !== intent.owner ||
      saved.path !== intent.path ||
      saved.publicId !== intent.publicId ||
      saved.expiresAt !== intent.expiresAt ||
      JSON.stringify(saved.body) !== JSON.stringify(intent.body) ||
      (!credentialRetry && intent.wireBody !== undefined && saved.wireBody !== intent.wireBody) ||
      (saved.key !== intent.key && !credentialRetry))
  )
    throw new IntentConflictError(saved);
  const submitted =
    intent.submitted ||
    saved?.recoveryRequired === true ||
    (saved?.submitted === true && intent.credentialRecovery !== 'refused' && !credentialRetry);
  const sameCommand = saved?.id === intent.id && saved.key === intent.key;
  const wireBody = intent.wireBody ?? (sameCommand ? saved.wireBody : undefined);
  const recoveryRequired =
    intent.recoveryRequired ?? (sameCommand ? saved.recoveryRequired : undefined);
  sessionStorage.setItem(KEY, JSON.stringify({ ...intent, submitted, wireBody, recoveryRequired }));
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
      intent.body?.start_if_ready !== true ||
      Object.keys(intent.body).some(
        (field) => !['start_if_ready', 'placement_id', 'referrer_origin', 'choice'].includes(field),
      ) ||
      (intent.wireBody !== undefined &&
        (typeof intent.wireBody !== 'string' || intent.wireBody !== JSON.stringify(intent.body))) ||
      (intent.recoveryRequired !== undefined &&
        (intent.recoveryRequired !== true || !intent.submitted))
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
      wireBody: intent.wireBody,
      recoveryRequired: intent.recoveryRequired,
      owner: intent.owner,
      credentialRecovery: intent.credentialRecovery,
      body: intent.body,
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
