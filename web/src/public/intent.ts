import type { CreateLaunchRequest, LaunchRepeatChoice } from '@chronoai/cma-sdk';
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
}

export function writeIntent(intent: BrowserIntent): void {
  sessionStorage.setItem(KEY, JSON.stringify(intent));
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
    const raw: unknown = JSON.parse(sessionStorage.getItem(KEY) ?? 'null');
    if (typeof raw !== 'object' || raw === null) return undefined;
    const intent = raw as Partial<BrowserIntent>;
    if (
      intent.path !== path ||
      typeof intent.id !== 'string' ||
      typeof intent.key !== 'string' ||
      typeof intent.publicId !== 'string' ||
      typeof intent.expiresAt !== 'number' ||
      intent.expiresAt <= Date.now() ||
      intent.expiresAt > Date.now() + LIFETIME ||
      typeof intent.submitted !== 'boolean' ||
      intent.body?.start_if_ready !== true
    )
      return undefined;
    if (
      (intent.owner !== undefined && typeof intent.owner !== 'string') ||
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
    if (canonical !== path) return undefined;
    return {
      id: intent.id,
      key: intent.key,
      path,
      publicId: intent.publicId,
      expiresAt: intent.expiresAt,
      submitted: intent.submitted,
      owner: intent.owner,
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
