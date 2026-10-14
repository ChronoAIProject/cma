export interface AgentImageChoice {
  readonly id: string;
  readonly name: string;
  readonly description?: string;
  readonly owner?: string;
  readonly visibility?: 'private' | 'shared' | 'public';
  readonly retired?: boolean;
  readonly revision?: number;
}

export function imageVisibilityLabel(visibility: AgentImageChoice['visibility']): string {
  if (visibility === 'public') return 'Public';
  return visibility === 'shared' ? 'Shared with you' : 'Private';
}

export interface ResolvedAgentImage {
  readonly id: string;
  readonly version: string;
  readonly reference: string;
}

interface ImagePage<T> {
  readonly items?: readonly T[];
  readonly next_cursor?: string | null;
}

/**
 * Image selection belongs to Image Manager. The Agent Manager wizard uses a
 * read-only catalogue query and always keeps the system image as a safe
 * choice when the independent service is unavailable.
 */
export async function listAgentImages(signal?: AbortSignal): Promise<readonly AgentImageChoice[]> {
  return imagePages<AgentImageChoice>('/api/image-manager/v1/images?scope=all', signal);
}

/** Resolve the newest successful immutable OCI version for a selected image. */
export async function resolveAgentImage(
  id: string,
  signal?: AbortSignal,
): Promise<ResolvedAgentImage> {
  const versions = await imagePages<{
    readonly id?: string;
    readonly phase?: string;
    readonly reference?: string;
    readonly created_at_ms?: number;
  }>(`/api/image-manager/v1/images/${encodeURIComponent(id)}/versions`, signal);
  const build = versions
    .filter(
      (item) => item.phase === 'succeeded' && item.id !== undefined && item.reference !== undefined,
    )
    .sort((left, right) => (right.created_at_ms ?? 0) - (left.created_at_ms ?? 0))[0];
  if (build?.id === undefined || build.reference === undefined)
    throw new Error('The selected image has no successful immutable version.');
  return { id, version: build.id, reference: build.reference };
}

async function imagePages<T>(path: string, signal?: AbortSignal): Promise<readonly T[]> {
  const items: T[] = [];
  const cursors = new Set<string>();
  let after = '';
  for (let page = 0; page < 100; page += 1) {
    const response = await fetch(
      `${path}${after ? `${path.includes('?') ? '&' : '?'}after=${encodeURIComponent(after)}` : ''}`,
      {
        credentials: 'same-origin',
        cache: 'no-store',
        signal,
      },
    );
    if (!response.ok) throw new Error(`Image Manager is unavailable (${String(response.status)}).`);
    const value = (await response.json()) as ImagePage<T>;
    if (!Array.isArray(value.items))
      throw new Error('Image Manager returned an invalid catalog. Refresh and try again.');
    if (value.items.length > 10_000 - items.length)
      throw new Error('The image catalog is too large to load. Contact your administrator.');
    items.push(...value.items);
    if (value.next_cursor === undefined || value.next_cursor === null || value.next_cursor === '')
      return items;
    if (
      typeof value.next_cursor !== 'string' ||
      value.next_cursor.length > 4096 ||
      cursors.has(value.next_cursor)
    )
      throw new Error('Image Manager returned an incomplete catalog. Refresh and try again.');
    after = value.next_cursor;
    cursors.add(after);
  }
  throw new Error('The image catalog is too large to load. Contact your administrator.');
}
