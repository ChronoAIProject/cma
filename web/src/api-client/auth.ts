import { headersFor, observeResponse, urlOf, type ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import {
  errorFromResponse,
  errorFromBody,
  readJsonBody,
  throwIfSignalAborted,
  withDeadline,
  type OperationOptions,
} from './http.js';
import { query, read } from './resources.js';
import {
  array,
  boolean,
  enumeration,
  object,
  optional,
  record,
  text,
  type Output,
} from './schema.js';

/** Complete encoded absolute URL budget; mirrored from the shared browser-auth-limits wire fixture. */
export const MAX_LOGIN_URL_BYTES = 4096;

const capability = enumeration(
  'create_sandbox',
  'use_own_sandboxes',
  'delete_own_sandbox',
  'manage_own_profiles',
  'manage_own_bots',
  'publish_triggers',
  'launch_triggers',
  'read_all_sandboxes',
  'control_any_sandbox',
  'read_runtime_inventory',
  'read_event_log',
  'read_activity_log',
  'read_user_directory',
  'configure_platform',
);
export type Capability = Output<typeof capability>;
/** Terms the deployment publishes; present signed in or out only when configured. */
const legalTerms = optional(object({ version: text, terms_url: text, privacy_url: text }));
const signedOut = object({ legal_terms: legalTerms });
const signedIn = object({
  api_commands: optional(boolean),
  privacy_export: optional(boolean),
  usage_reporting: optional(object({ measured_visible: boolean })),
  subject: text,
  capabilities: array(capability),
  visitor: boolean,
  display_name: optional(text),
  email: optional(text),
  avatar_url: optional(text),
  legal_terms: legalTerms,
});
export type AuthSession =
  | ({ readonly authenticated: false } & Output<typeof signedOut>)
  | ({ readonly authenticated: true } & Output<typeof signedIn>);
const signupOptions = object({
  social_providers: array(text),
  email_signup: boolean,
  invite_code_required: boolean,
  register_available: boolean,
});
export type SignupOptions = Output<typeof signupOptions>;
export interface LoginOptions {
  readonly returnTo?: string;
  readonly screen?: 'register';
}

/** Browser identity inspection. Login and logout are explicit navigation URLs, not background fetches. */
export class Authentication {
  constructor(private readonly config: ResolvedConfig) {}

  readSession(options: OperationOptions = {}): Promise<AuthSession> {
    return withDeadline(this.config, options, async (signal, requestId) => {
      const headers = await headersFor(this.config, undefined, { requestId });
      throwIfSignalAborted(signal);
      const response = await this.config.fetch(urlOf(this.config, '/auth/session'), {
        method: 'GET',
        headers,
        credentials: this.config.cookies ? 'include' : 'omit',
        redirect: 'error',
        signal,
      });
      const metadata = await observeResponse(this.config, response);
      if (![200, 401, 503].includes(response.status))
        throw await errorFromResponse(response, signal, metadata.requestId);
      const body = record(await readJsonBody(response, signal));
      if (response.status === 503) {
        const problem =
          body.authenticated === false && body.code === 'store_failed'
            ? { status: 503, code: 'store_failed', detail: 'Browser identity could not be read.' }
            : body;
        throw errorFromBody(response, problem, metadata.requestId);
      }
      if (response.status === 401 && body.authenticated === false)
        return { authenticated: false, ...signedOut(body) };
      if (response.status !== 200 || body.authenticated !== true)
        throw new ProtocolError('Authentication status did not match its body');
      return { authenticated: true, ...signedIn(body) };
    });
  }

  readSignupOptions(options: OperationOptions = {}) {
    return read(
      { ...this.config, credential: undefined, cookies: false },
      '/auth/signup-options',
      signupOptions,
      options,
    );
  }

  loginUrl(options: LoginOptions = {}): string {
    if (options.screen !== undefined && options.screen !== 'register')
      throw new ValidationError('screen must be register when supplied');
    if (options.returnTo !== undefined) validateReturnTarget(options.returnTo);
    const target = urlOf(
      this.config,
      query('/auth/login', { return_to: options.returnTo, screen: options.screen }),
    );
    if (target.length > MAX_LOGIN_URL_BYTES)
      throw new ValidationError('encoded login URL exceeds the public navigation budget');
    return target;
  }

  logoutUrl(): string {
    return urlOf(this.config, '/auth/logout');
  }
}

function validateReturnTarget(value: string): void {
  if (typeof value !== 'string')
    throw new ValidationError('returnTo must be a canonical CMA workflow path');
  const split = value.indexOf('?');
  const path = split < 0 ? value : value.slice(0, split);
  const query = split < 0 ? undefined : value.slice(split + 1);
  const id = '[A-Za-z0-9_.~-]+';
  const workflow = new RegExp(
    `^/(?:agent-manager|event-gateway|image-manager|bots(?:/${id})?|auth/bot-father/link/${id}/complete|t/${id}(?:/${id})?|triggers/manage(?:/${id}(?:/embed)?)?|agents/${id}|sandboxes/${id}|(?:profiles|marketplace)(?:/(?:agent|workspace|sandbox)/${id})?)$`,
  );
  const keys = new Set<string>();
  let validQuery = true;
  for (const [key] of new URLSearchParams(query)) {
    if (
      keys.has(key) ||
      !(
        (path === '/bots/new' && key === 'setup') ||
        (path.startsWith('/sandboxes/') && key === 'tab')
      )
    )
      validQuery = false;
    keys.add(key);
  }
  if (
    value.length > MAX_LOGIN_URL_BYTES ||
    path.length > 256 ||
    Array.from(value).some(
      (character) =>
        character.charCodeAt(0) <= 32 ||
        character.charCodeAt(0) >= 127 ||
        character === '#' ||
        character === '\\',
    ) ||
    !validQuery ||
    path.split('/').some((segment) => segment === '.' || segment === '..') ||
    (path !== '/' && !workflow.test(path))
  )
    throw new ValidationError('returnTo must be a canonical CMA workflow path');
}
