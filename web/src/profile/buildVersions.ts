import { API_BASE_URL, BUILD_INFO } from '../shell/build';
import { isPromotableVersion } from '../shell/releaseVersion';

interface ServiceBuild {
  readonly version: string;
  readonly source_sha: string;
}

type ServiceBuilds = {
  readonly am: ServiceBuild | undefined;
  readonly eg: ServiceBuild | undefined;
};

export const BUILD_LOOKUP_TIMEOUT_MS = 2500;

function serviceBuild(value: unknown): ServiceBuild | undefined {
  if (typeof value !== 'object' || value === null) return;
  const { version, source_sha } = value as Partial<ServiceBuild>;
  if (
    typeof version === 'string' &&
    typeof source_sha === 'string' &&
    /^[a-f0-9]{40}$/.test(source_sha) &&
    /[a-f1-9]/.test(source_sha) &&
    isPromotableVersion(version, source_sha)
  ) {
    return { version, source_sha };
  }
}

async function readBuild(
  source: 'am' | 'eg',
  signal: AbortSignal,
): Promise<ServiceBuild | undefined> {
  const path = source === 'am' ? '/api/v1/build-info' : '/event-gateway/v1/readyz';
  try {
    const response = await fetch(new URL(path, API_BASE_URL), {
      cache: 'no-store',
      credentials: 'omit',
      signal,
    });
    if (!response.ok && !(source === 'eg' && response.status === 503)) return;
    const data: unknown = await response.json();
    if (source === 'am') return serviceBuild(data);
    if (typeof data === 'object' && data !== null && 'ready' in data && 'build' in data) {
      if (data.ready === response.ok) return serviceBuild(data.build);
    }
  } catch {
    // An unreachable service or an unrelated proxy response has no build identity.
    return undefined;
  }
}

export async function readServiceBuilds(signal: AbortSignal): Promise<ServiceBuilds> {
  const [am, eg] = await Promise.all([readBuild('am', signal), readBuild('eg', signal)]);
  return { am, eg };
}

export function describeBuilds(builds: ServiceBuilds | undefined) {
  const components = [
    ['AM', builds?.am],
    ['EG', builds?.eg],
  ] as const;
  const label = [
    `Web ${BUILD_INFO.label}`,
    ...components.map(
      ([name, build]) => `${name} ${build?.version ?? (builds ? 'unavailable' : 'checking…')}`,
    ),
  ].join(' · ');
  const { version, revision, local } = BUILD_INFO;
  const copyText = [
    `Web ${version} (${revision || 'local'})${local && revision ? ' (local)' : ''}`,
    ...components.map(([name, build]) =>
      build ? `${name} ${build.version} (${build.source_sha})` : `${name} unavailable`,
    ),
  ].join(' · ');
  return { label, copyText };
}
