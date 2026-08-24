/** Environment profiles and the catalogues used to author them. */

import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import { request, requestPages, requestVoid, type OperationOptions } from './http.js';
import type { CredentialMode, PermissionMode, ReasoningEffort, SessionPlane } from './types.js';

const BASE = '/api/v1/environment-profiles';
const UINT32_MAX = 4_294_967_295;

const PLANES: readonly SessionPlane[] = ['terminal', 'conversation'];
const EFFORTS: readonly ReasoningEffort[] = ['minimal', 'low', 'medium', 'high', 'x_high'];
const PERMISSIONS: readonly PermissionMode[] = ['guarded', 'autonomous', 'unrestricted'];
const CREDENTIALS: readonly CredentialMode[] = ['delegated', 'own'];

export interface ProfileVariable {
  readonly key: string;
  readonly value: string;
}

export interface ProfilePackage {
  readonly name: string;
  readonly version?: string;
}

export interface ProfileSessionSize {
  readonly cpuMillis: number;
  readonly memoryMib: number;
}

/** How sessions created from a stored profile are configured. */
export interface EnvironmentAgent {
  readonly kernel: string;
  readonly mode: SessionPlane;
  readonly model?: string;
  readonly reasoningEffort: ReasoningEffort;
  readonly permission: PermissionMode;
  readonly credential: CredentialMode;
  readonly size?: ProfileSessionSize;
  /** Optional until every supported control plane includes the #485 response field. */
  readonly recordToolBodies?: boolean;
  readonly skills: readonly string[];
}

/** Agent fields accepted on create and full replacement. Omitted fields use deployment defaults. */
export interface EnvironmentAgentDraft {
  readonly kernel?: string;
  readonly mode?: SessionPlane;
  readonly model?: string;
  readonly reasoningEffort?: ReasoningEffort;
  readonly permission?: PermissionMode;
  readonly credential?: CredentialMode;
  readonly size?: ProfileSessionSize;
  readonly recordToolBodies?: boolean;
  readonly skills?: readonly string[];
}

export interface NyxIdAccess {
  readonly services: readonly string[];
}

export interface NyxIdAccessDraft {
  readonly services?: readonly string[];
}

export interface MandatoryEntries {
  readonly packages: readonly string[];
  readonly skills: readonly string[];
}

/** Full-document input for profile creation and replacement. */
export interface EnvironmentProfileDraft {
  readonly name: string;
  readonly variables?: readonly ProfileVariable[];
  readonly packages?: readonly ProfilePackage[];
  readonly agent?: EnvironmentAgentDraft;
  readonly nyxid?: NyxIdAccessDraft;
  readonly agentNotes?: string;
}

export interface EnvironmentProfile {
  readonly id: string;
  readonly name: string;
  readonly variables: readonly ProfileVariable[];
  readonly packages: readonly ProfilePackage[];
  readonly agent: EnvironmentAgent;
  readonly nyxid: NyxIdAccess;
  readonly agentNotes: string;
  readonly mandatory: MandatoryEntries;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly preferred: boolean;
}

export interface CatalogSkill {
  readonly id: string;
  readonly name: string;
  readonly description: string;
  readonly access?: string;
}

export interface SkillCatalogue {
  readonly skills: readonly CatalogSkill[];
  readonly mandatory: MandatoryEntries;
}

export interface PlatformService {
  readonly id: string;
  readonly name: string;
  readonly description?: string;
}

export interface ServiceCatalogue {
  readonly services: readonly PlatformService[];
  readonly mandatory: readonly string[];
}

/** Every profile owned by the authenticated subject. */
export async function listEnvironmentProfiles(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<readonly EnvironmentProfile[]> {
  return requestPages(
    config,
    BASE,
    (body) => profileList(body, 'environment profile list'),
    options,
  );
}

/** Read one owned profile. */
export async function readEnvironmentProfile(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
): Promise<EnvironmentProfile> {
  return toEnvironmentProfile(
    await request(config, profilePath(id), { ...options, expectedStatus: 200 }),
  );
}

/** Store a profile. */
export async function createEnvironmentProfile(
  config: ResolvedConfig,
  draft: EnvironmentProfileDraft,
  options: OperationOptions = {},
): Promise<EnvironmentProfile> {
  return toEnvironmentProfile(
    await request(config, BASE, {
      ...options,
      method: 'POST',
      expectedStatus: 201,
      body: profileBody(draft),
    }),
  );
}

/** Replace every caller-controlled field on one profile. */
export async function updateEnvironmentProfile(
  config: ResolvedConfig,
  id: string,
  draft: EnvironmentProfileDraft,
  options: OperationOptions = {},
): Promise<EnvironmentProfile> {
  return toEnvironmentProfile(
    await request(config, profilePath(id), {
      ...options,
      method: 'PUT',
      expectedStatus: 200,
      body: profileBody(draft),
    }),
  );
}

/** Delete one profile. */
export async function deleteEnvironmentProfile(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
): Promise<void> {
  await requestVoid(config, profilePath(id), {
    ...options,
    method: 'DELETE',
    expectedStatus: 204,
  });
}

/** Choose the default profile, or clear the choice with `null`. */
export async function preferEnvironmentProfile(
  config: ResolvedConfig,
  id: string | null,
  options: OperationOptions = {},
): Promise<readonly EnvironmentProfile[]> {
  if (id !== null) {
    requireIdentifier(id, 'environment profile id');
  }
  return profileList(
    await request(config, `${BASE}/preferred`, {
      ...options,
      method: 'PUT',
      expectedStatus: 200,
      body: { profile: id },
    }),
    'preferred profile response',
  );
}

/** Skills the subject may select and the entries a profile cannot remove. */
export async function readSkillCatalogue(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<SkillCatalogue> {
  const body = objectOf(
    await request(config, `${BASE}/skills`, { ...options, expectedStatus: 200 }),
    'skill catalogue',
  );
  return {
    skills: objectArray(body.skills, 'skill catalogue skills').map((skill) => ({
      id: nonEmptyString(skill.id, 'skill id'),
      name: requiredString(skill.name, 'skill name'),
      description: requiredString(skill.description, 'skill description'),
      access: optionalString(skill.access, 'skill access'),
    })),
    mandatory: mandatoryOf(body.mandatory, 'skill catalogue mandatory entries'),
  };
}

/** NyxID services the subject may grant and the entries a profile cannot remove. */
export async function readServiceCatalogue(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<ServiceCatalogue> {
  const body = objectOf(
    await request(config, `${BASE}/services`, { ...options, expectedStatus: 200 }),
    'service catalogue',
  );
  return {
    services: objectArray(body.services, 'service catalogue services').map((service) => ({
      id: nonEmptyString(service.id, 'service id'),
      name: requiredString(service.name, 'service name'),
      description: optionalString(service.description, 'service description'),
    })),
    mandatory: stringArray(body.mandatory, 'service catalogue mandatory entries'),
  };
}

/** Validate one profile response at the SDK boundary. */
export function toEnvironmentProfile(raw: unknown): EnvironmentProfile {
  const body = objectOf(raw, 'environment profile');
  const agent = objectOf(body.agent, 'environment profile agent');
  const nyxid = objectOf(body.nyxid, 'environment profile nyxid access');
  return {
    id: nonEmptyString(body.id, 'environment profile id'),
    name: requiredString(body.name, 'environment profile name'),
    variables: objectArray(body.variables, 'environment profile variables').map((variable) => ({
      key: requiredString(variable.key, 'environment variable key'),
      value: requiredString(variable.value, 'environment variable value'),
    })),
    packages: objectArray(body.packages, 'environment profile packages').map((software) => ({
      name: requiredString(software.name, 'environment package name'),
      version: optionalString(software.version, 'environment package version'),
    })),
    agent: {
      kernel: nonEmptyString(agent.kernel, 'environment agent kernel'),
      mode: memberOf(agent.mode, PLANES, 'environment agent mode'),
      model: optionalString(agent.model, 'environment agent model'),
      reasoningEffort: memberOf(
        agent.reasoning_effort,
        EFFORTS,
        'environment agent reasoning_effort',
      ),
      permission: memberOf(agent.permission, PERMISSIONS, 'environment agent permission'),
      credential: memberOf(agent.credential, CREDENTIALS, 'environment agent credential'),
      size: optionalSize(agent.size, ProtocolError),
      recordToolBodies: optionalBoolean(
        agent.record_tool_bodies,
        'environment agent record_tool_bodies',
      ),
      skills: stringArray(agent.skills, 'environment agent skills'),
    },
    nyxid: { services: stringArray(nyxid.services, 'environment profile services') },
    agentNotes: requiredString(body.agent_notes, 'environment profile agent_notes'),
    mandatory: mandatoryOf(body.mandatory, 'environment profile mandatory entries'),
    createdAt: nonEmptyString(body.created_at, 'environment profile created_at'),
    updatedAt: nonEmptyString(body.updated_at, 'environment profile updated_at'),
    preferred: requiredBoolean(body.preferred, 'environment profile preferred'),
  };
}

function profileBody(draft: EnvironmentProfileDraft): Record<string, unknown> {
  const body = validationObject(draft, 'environment profile draft');
  const agent = optionalValidationObject(body.agent, 'environment profile agent draft');
  const nyxid = optionalValidationObject(body.nyxid, 'environment profile nyxid draft');
  return {
    name: validationString(body.name, 'environment profile name'),
    variables: validationObjects(body.variables, 'environment profile variables').map(
      (variable) => ({
        key: validationString(variable.key, 'environment variable key'),
        value: validationString(variable.value, 'environment variable value'),
      }),
    ),
    packages: validationObjects(body.packages, 'environment profile packages').map((software) => ({
      name: validationString(software.name, 'environment package name'),
      version: validationOptionalString(software.version, 'environment package version'),
    })),
    agent:
      agent === undefined
        ? undefined
        : {
            kernel: validationOptionalString(agent.kernel, 'environment agent kernel'),
            mode: validationOptionalMember(agent.mode, PLANES, 'environment agent mode'),
            model: validationOptionalString(agent.model, 'environment agent model'),
            reasoning_effort: validationOptionalMember(
              agent.reasoningEffort,
              EFFORTS,
              'environment agent reasoning effort',
            ),
            permission: validationOptionalMember(
              agent.permission,
              PERMISSIONS,
              'environment agent permission',
            ),
            credential: validationOptionalMember(
              agent.credential,
              CREDENTIALS,
              'environment agent credential',
            ),
            size: sizeBody(agent.size),
            record_tool_bodies: validationOptionalBoolean(
              agent.recordToolBodies,
              'environment agent recordToolBodies',
            ),
            skills: validationStrings(agent.skills, 'environment agent skills'),
          },
    nyxid:
      nyxid === undefined
        ? undefined
        : { services: validationStrings(nyxid.services, 'environment profile services') },
    agent_notes: validationOptionalString(body.agentNotes, 'environment profile agent notes'),
  };
}

function profilePath(id: string): string {
  requireIdentifier(id, 'environment profile id');
  return `${BASE}/${encodeURIComponent(id)}`;
}

function requireIdentifier(value: string, name: string): void {
  if (typeof value !== 'string' || value === '') {
    throw new ValidationError(`${name} must be a non-empty string`);
  }
}

function profileList(raw: unknown, name: string): readonly EnvironmentProfile[] {
  if (!Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not a list`);
  }
  return raw.map(toEnvironmentProfile);
}

function mandatoryOf(raw: unknown, name: string): MandatoryEntries {
  const body = objectOf(raw, name);
  return {
    packages: stringArray(body.packages, `${name} packages`),
    skills: stringArray(body.skills, `${name} skills`),
  };
}

function optionalSize(
  raw: unknown,
  ErrorType: typeof ProtocolError | typeof ValidationError,
): ProfileSessionSize | undefined {
  if (raw === undefined || raw === null) {
    return undefined;
  }
  const size =
    ErrorType === ProtocolError
      ? objectOf(raw, 'environment agent size')
      : validationObject(raw, 'environment agent size');
  const cpuMillis = size.cpu_millis ?? size.cpuMillis;
  const memoryMib = size.memory_mib ?? size.memoryMib;
  if (!positiveUint32(cpuMillis) || !positiveUint32(memoryMib)) {
    throw new ErrorType(
      'environment agent size requires positive uint32 CPU millis and memory MiB',
    );
  }
  return { cpuMillis, memoryMib };
}

function sizeBody(raw: unknown): Record<string, number> | undefined {
  const size = optionalSize(raw, ValidationError);
  return size === undefined
    ? undefined
    : { cpu_millis: size.cpuMillis, memory_mib: size.memoryMib };
}

function positiveUint32(raw: unknown): raw is number {
  return typeof raw === 'number' && Number.isSafeInteger(raw) && raw > 0 && raw <= UINT32_MAX;
}

function objectOf(raw: unknown, name: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not an object`);
  }
  return raw as Record<string, unknown>;
}

function objectArray(raw: unknown, name: string): readonly Record<string, unknown>[] {
  if (!Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not a list`);
  }
  return raw.map((entry) => objectOf(entry, `${name} entry`));
}

function requiredString(raw: unknown, name: string): string {
  if (typeof raw !== 'string') {
    throw new ProtocolError(`${name} was not text`);
  }
  return raw;
}

function nonEmptyString(raw: unknown, name: string): string {
  const value = requiredString(raw, name);
  if (value === '') {
    throw new ProtocolError(`${name} was empty`);
  }
  return value;
}

function optionalString(raw: unknown, name: string): string | undefined {
  return raw === undefined || raw === null ? undefined : requiredString(raw, name);
}

function stringArray(raw: unknown, name: string): readonly string[] {
  if (!Array.isArray(raw) || !raw.every((entry) => typeof entry === 'string')) {
    throw new ProtocolError(`${name} was not a list of strings`);
  }
  return raw;
}

function requiredBoolean(raw: unknown, name: string): boolean {
  if (typeof raw !== 'boolean') {
    throw new ProtocolError(`${name} was not boolean`);
  }
  return raw;
}

function optionalBoolean(raw: unknown, name: string): boolean | undefined {
  if (raw === undefined || raw === null) {
    return undefined;
  }
  return requiredBoolean(raw, name);
}

function memberOf<T extends string>(raw: unknown, values: readonly T[], name: string): T {
  if (typeof raw !== 'string' || !values.includes(raw as T)) {
    throw new ProtocolError(`${name} used an unrecognised value`);
  }
  return raw as T;
}

function validationObject(raw: unknown, name: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ValidationError(`${name} must be an object`);
  }
  return raw as Record<string, unknown>;
}

function optionalValidationObject(raw: unknown, name: string): Record<string, unknown> | undefined {
  return raw === undefined ? undefined : validationObject(raw, name);
}

function validationString(raw: unknown, name: string): string {
  if (typeof raw !== 'string') {
    throw new ValidationError(`${name} must be a string`);
  }
  return raw;
}

function validationOptionalString(raw: unknown, name: string): string | undefined {
  return raw === undefined ? undefined : validationString(raw, name);
}

function validationObjects(raw: unknown, name: string): readonly Record<string, unknown>[] {
  if (raw === undefined) {
    return [];
  }
  if (!Array.isArray(raw)) {
    throw new ValidationError(`${name} must be a list`);
  }
  return raw.map((entry) => validationObject(entry, `${name} entry`));
}

function validationStrings(raw: unknown, name: string): readonly string[] {
  if (raw === undefined) {
    return [];
  }
  if (!Array.isArray(raw) || !raw.every((entry) => typeof entry === 'string')) {
    throw new ValidationError(`${name} must be a list of strings`);
  }
  return raw;
}

function validationOptionalBoolean(raw: unknown, name: string): boolean | undefined {
  if (raw !== undefined && typeof raw !== 'boolean') {
    throw new ValidationError(`${name} must be boolean`);
  }
  return raw;
}

function validationOptionalMember<T extends string>(
  raw: unknown,
  values: readonly T[],
  name: string,
): T | undefined {
  if (raw === undefined) {
    return undefined;
  }
  if (typeof raw !== 'string' || !values.includes(raw as T)) {
    throw new ValidationError(`${name} used an unrecognised value`);
  }
  return raw as T;
}
