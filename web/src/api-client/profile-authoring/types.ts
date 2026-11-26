import { ProtocolError } from '../errors.js';
import { array, object, optional, positiveInteger, text, type Output } from '../schema.js';

const byteLength = (value: string) => new TextEncoder().encode(value).length;
const hasControls = (value: string, asciiOnly = false) =>
  [...value].some((character) => {
    const code = character.codePointAt(0)!;
    return code < 32 || (code >= 127 && code <= (asciiOnly ? 127 : 159));
  });

function boundedText(max: number) {
  return (value: unknown): string => {
    const result = text(value);
    if (!result || result.trim() !== result || byteLength(result) > max || hasControls(result))
      throw new ProtocolError('invalid authoring catalog text');
    return result;
  };
}

const description = (value: unknown): string => {
  const result = text(value);
  if (byteLength(result) > 6144 || result.includes('\0'))
    throw new ProtocolError('invalid authoring catalog description');
  return result;
};

export const cursor = (value: unknown): string => {
  const result = text(value);
  if (!result || byteLength(result) > 2048 || hasControls(result))
    throw new ProtocolError('cursor must be nonempty, at most 2048 bytes, and contain no controls');
  return result;
};

const capacity = (value: unknown): number => {
  const result = positiveInteger(value);
  if (result > 0xffff_ffff) throw new ProtocolError('capacity exceeds its unsigned 32-bit bound');
  return result;
};
const boundsShape = object({
  min_cpu_millis: capacity,
  max_cpu_millis: capacity,
  min_memory_mib: capacity,
  max_memory_mib: capacity,
});
export type ProfileAuthoringBounds = Output<typeof boundsShape>;
export function bounds(value: unknown): ProfileAuthoringBounds {
  const result = boundsShape(value);
  if (
    result.min_cpu_millis > result.max_cpu_millis ||
    result.min_memory_mib > result.max_memory_mib
  )
    throw new ProtocolError('profile authoring capacity bounds are reversed');
  return result;
}

const service = object({
  id: boundedText(128),
  catalog_id: optional(boundedText(128)),
  detail_url: optional(boundedText(2048)),
  name: boundedText(256),
  description: optional(description),
});
export type ProfileAuthoringService = Output<typeof service>;
export function services(value: unknown): readonly ProfileAuthoringService[] {
  const result = array(service)(value);
  if (result.length > 1000 || new Set(result.map((entry) => entry.id)).size !== result.length)
    throw new ProtocolError('service catalog exceeds its bound or repeats an identity');
  return result;
}

const version = (value: unknown): string => {
  const result = text(value);
  if (result.length > 20 || !/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(result))
    throw new ProtocolError('skill versions must be canonical literal major.minor strings');
  return result;
};
const skillId = (value: unknown): string => {
  const result = text(value);
  if (!/^[a-zA-Z0-9_-]{1,64}$/.test(result)) throw new ProtocolError('invalid skill identity');
  return result;
};
const skill = object({
  id: skillId,
  name: boundedText(256),
  description: optional(description),
  versions: array(version),
});
export type ProfileAuthoringSkill = Output<typeof skill>;
const skillPage = object({ items: array(skill), next: optional(cursor) });
export type ProfileAuthoringSkillPage = Output<typeof skillPage>;

export interface ProfileAuthoringSkillSearch {
  readonly q?: string;
  readonly limit?: number;
  readonly cursor?: string;
}

const searchShape = object({
  q: optional(text),
  limit: optional(positiveInteger),
  cursor: optional(cursor),
});
export function validateSearch(value: unknown): ProfileAuthoringSkillSearch {
  const result = searchShape(value);
  if (result.q !== undefined && (byteLength(result.q) > 200 || hasControls(result.q)))
    throw new ProtocolError('skill search q must be at most 200 UTF-8 bytes without controls');
  if (result.limit !== undefined && result.limit > 50)
    throw new ProtocolError('skill search limit must be 1 to 50');
  return result;
}

function descending(left: string, right: string): boolean {
  const [leftMajor, leftMinor] = left.split('.') as [string, string];
  const [rightMajor, rightMinor] = right.split('.') as [string, string];
  const compare = (a: string, b: string) => a.length - b.length || (a > b ? 1 : a < b ? -1 : 0);
  return (compare(leftMajor, rightMajor) || compare(leftMinor, rightMinor)) > 0;
}

export function skills(
  value: unknown,
  query: ProfileAuthoringSkillSearch,
): ProfileAuthoringSkillPage {
  const result = skillPage(value);
  if (
    result.items.length > (query.limit ?? 20) ||
    new Set(result.items.map((entry) => entry.id)).size !== result.items.length ||
    (result.next !== undefined && result.next === query.cursor) ||
    result.items.some(
      (entry) =>
        entry.versions.length === 0 ||
        entry.versions.length > 100 ||
        entry.versions.some(
          (value, index) => index > 0 && !descending(entry.versions[index - 1]!, value),
        ),
    )
  )
    throw new ProtocolError('invalid profile authoring skill page or version order');
  return result;
}

export const repositoryRequest = object({ url: text, reference: optional(text) });
export type ResolveRepositoryRequest = Output<typeof repositoryRequest>;
const repositoryShape = object({
  url: text,
  reference: optional(text),
  default_branch: text,
  commit_sha: text,
});
export type ResolvedRepository = Output<typeof repositoryShape>;

function repositoryIdentity(value: string): string | undefined {
  const prefix = 'https://github.com/';
  if (!value.startsWith(prefix)) return undefined;
  const parts = value.slice(prefix.length).replace(/\/$/, '').split('/');
  if (parts.length !== 2) return undefined;
  const owner = parts[0]!;
  const repository = parts[1]!.replace(/\.git$/, '');
  const component = (value: string) =>
    /^[a-zA-Z0-9_.-]{1,100}$/.test(value) && value !== '.' && value !== '..';
  return component(owner) && component(repository)
    ? `${owner}/${repository}`.toLowerCase()
    : undefined;
}

export function repository(value: unknown, request: ResolveRepositoryRequest): ResolvedRepository {
  const result = repositoryShape(value);
  const identity = repositoryIdentity(result.url);
  if (
    identity === undefined ||
    identity !== repositoryIdentity(request.url) ||
    result.reference !== request.reference ||
    !result.default_branch ||
    byteLength(result.default_branch) > 255 ||
    result.default_branch.startsWith('-') ||
    /\s/u.test(result.default_branch) ||
    hasControls(result.default_branch, true) ||
    !/^[0-9a-f]{40}$/.test(result.commit_sha)
  )
    throw new ProtocolError('repository resolution identity or commit did not match its contract');
  return result;
}
