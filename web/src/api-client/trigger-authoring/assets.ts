import { headersFor, observeResponse, urlOf, type ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import {
  errorFromResponse,
  readBytes,
  withDeadline,
  throwIfSignalAborted,
  type OperationOptions,
} from '../http.js';
import { input, query, segment } from '../resources.js';
import {
  triggerButtonLabel,
  triggerTheme,
  type TriggerButtonLabel,
  type TriggerTheme,
} from './types.js';

export interface TriggerBadgeOptions extends OperationOptions {
  readonly label?: TriggerButtonLabel;
  readonly theme?: TriggerTheme;
  readonly placement?: string;
  readonly ifNoneMatch?: string;
}
export type TriggerBadge =
  | { readonly status: 200; readonly svg: string; readonly etag: string }
  | { readonly status: 304; readonly etag: string };

/** Read an inert public badge without resolving credentials or sending cookies. */
export function readTriggerBadge(
  config: ResolvedConfig,
  publicId: string,
  options: TriggerBadgeOptions = {},
): Promise<TriggerBadge> {
  if (options.label !== undefined) input(triggerButtonLabel, options.label);
  if (options.theme !== undefined) input(triggerTheme, options.theme);
  if (options.placement !== undefined && !/^[A-Za-z0-9_-]{1,128}$/.test(options.placement))
    throw new ValidationError('placement must be a valid public identifier');
  if (options.ifNoneMatch !== undefined && !/^[\x20-\x7e]{1,1024}$/.test(options.ifNoneMatch))
    throw new ValidationError('ifNoneMatch must be a bounded HTTP entity-tag condition');
  const path = query(`/api/v1/triggers/public/${segment(publicId)}/badge.svg`, {
    label: options.label,
    theme: options.theme,
    placement: options.placement,
  });
  return anonymousAsset(
    config,
    path,
    options,
    async (response, signal) => {
      const etag = response.headers.get('etag');
      if (!etag) throw new ProtocolError('Trigger badge response carried no entity tag');
      if (response.status === 304) {
        if (options.ifNoneMatch === undefined)
          throw new ProtocolError('Unconditional badge read returned HTTP 304');
        if ((await readBytes(response, signal)).length !== 0)
          throw new ProtocolError('Unmodified badge carried a body');
        return { status: 304, etag };
      }
      requireMedia(response, ['image/svg+xml']);
      return { status: 200, svg: await utf8(response, signal), etag };
    },
    options.ifNoneMatch,
  );
}

/** Read the inert public script as text; this helper never executes it. */
export function readEmbedScript(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<string> {
  return anonymousAsset(config, '/embed/v1.js', options, async (response, signal) => {
    requireMedia(response, ['application/javascript', 'text/javascript']);
    return utf8(response, signal);
  });
}

function anonymousAsset<Value>(
  config: ResolvedConfig,
  path: string,
  options: OperationOptions,
  decode: (response: Response, signal: AbortSignal) => Promise<Value>,
  ifNoneMatch?: string,
): Promise<Value> {
  const anonymous = { ...config, credential: undefined, cookies: false };
  return withDeadline(anonymous, options, async (signal, requestId) => {
    const headers = await headersFor(
      anonymous,
      ifNoneMatch === undefined ? undefined : { 'If-None-Match': ifNoneMatch },
      { requestId },
    );
    throwIfSignalAborted(signal);
    const response = await anonymous.fetch(urlOf(anonymous, path), {
      method: 'GET',
      headers,
      credentials: 'omit',
      redirect: 'error',
      signal,
    });
    throwIfSignalAborted(signal);
    const metadata = await observeResponse(anonymous, response);
    if (response.status !== 200 && response.status !== 304)
      throw await errorFromResponse(response, signal, metadata.requestId);
    if (response.status === 304 && ifNoneMatch === undefined)
      throw new ProtocolError('Unconditional public asset read returned HTTP 304');
    return decode(response, signal);
  });
}

function requireMedia(response: Response, allowed: readonly string[]): void {
  const media = response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase();
  if (media === undefined || !allowed.includes(media))
    throw new ProtocolError('Public asset content type did not match the request');
}

async function utf8(response: Response, signal: AbortSignal): Promise<string> {
  const bytes = await readBytes(response, signal);
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch (cause) {
    throw new ProtocolError('Public asset was not valid UTF-8', { cause });
  }
}
