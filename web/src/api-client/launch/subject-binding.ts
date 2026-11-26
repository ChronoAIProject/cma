import type { ResolvedConfig } from '../config.js';
import { ApiError, ProtocolError, ValidationError } from '../errors.js';
import { errorFromResponse, readJsonBody, requireSuccess, send, withDeadline } from '../http.js';
import { command, input, segment, type CommandOptions } from '../resources.js';
import { createLaunchRequest, launch, type CreateLaunchRequest } from './types.js';

export interface BoundCreateLaunchOptions extends CommandOptions {
  readonly expectedSubject: string;
}

export class LaunchSubjectUnverifiedError extends ProtocolError {
  constructor() {
    super(
      'CMA could not verify the account for this response. Your original request is saved. Sign in with the original account before trying again.',
    );
  }
}

function encodeSubject(subject: string): string {
  if (typeof subject !== 'string' || subject.length === 0 || subject.length > 4096)
    throw new ValidationError('The expected command account cannot be represented.');
  const bytes = new TextEncoder().encode(subject);
  if (new TextDecoder().decode(bytes) !== subject)
    throw new ValidationError('The expected command account is not valid Unicode.');
  const encoded = `v1.${btoa(String.fromCharCode(...bytes))
    .replaceAll('+', '-')
    .replaceAll('/', '_')
    .replace(/=+$/, '')}`;
  if (encoded.length > 4096)
    throw new ValidationError('The expected command account cannot be represented.');
  return encoded;
}

export async function createBoundLaunch(
  config: ResolvedConfig,
  publicId: string,
  body: CreateLaunchRequest,
  options: BoundCreateLaunchOptions,
) {
  const expected = encodeSubject(options.expectedSubject);
  const request = command(options);
  const headers = new Headers(request.headers);
  headers.set('X-CMA-Expected-Subject', expected);
  return withDeadline(config, options, async (signal, requestId) => {
    const { response, metadata } = await send(
      config,
      `/api/v2/triggers/public/${segment(publicId)}/launches`,
      { ...request, method: 'POST', headers, body: input(createLaunchRequest, body) },
      signal,
      requestId,
    );
    if (response.headers.get('X-CMA-Verified-Subject') !== expected) {
      if (response.status === 409) {
        const error = await errorFromResponse(response, signal, metadata.requestId);
        if (error instanceof ApiError && error.code === 'command_subject_mismatch') throw error;
      }
      throw new LaunchSubjectUnverifiedError();
    }
    await requireSuccess(response, [201, 202], signal, metadata.requestId);
    return launch(await readJsonBody(response, signal));
  });
}
