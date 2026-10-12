export interface AgentImageChoice {
  readonly id: string;
  readonly name: string;
  readonly description?: string;
  readonly owner?: string;
  readonly visibility?: 'private' | 'shared';
  readonly retired?: boolean;
  readonly revision?: number;
}

export interface ResolvedAgentImage {
  readonly id: string;
  readonly version: string;
  readonly reference: string;
}

interface ImagePage {
  readonly items?: readonly AgentImageChoice[];
}

/**
 * Image selection belongs to Image Manager. The Agent Manager wizard uses a
 * read-only catalogue query and always keeps the system image as a safe
 * choice when the independent service is unavailable.
 */
export async function listAgentImages(signal?: AbortSignal): Promise<readonly AgentImageChoice[]> {
  const response = await fetch('/api/image-manager/v1/images?scope=all', {
    credentials: 'same-origin',
    cache: 'no-store',
    signal,
  });
  if (!response.ok) throw new Error(`Image Manager is unavailable (${String(response.status)}).`);
  const value = (await response.json()) as ImagePage;
  return Array.isArray(value.items) ? value.items : [];
}

/** Resolve the newest successful immutable OCI version for a selected image. */
export async function resolveAgentImage(
  id: string,
  signal?: AbortSignal,
): Promise<ResolvedAgentImage> {
  const response = await fetch(`/api/image-manager/v1/images/${encodeURIComponent(id)}/versions`, {
    credentials: 'same-origin',
    cache: 'no-store',
    signal,
  });
  if (!response.ok) throw new Error(`Image Manager is unavailable (${String(response.status)}).`);
  const value = (await response.json()) as {
    readonly items?: readonly {
      readonly id?: string;
      readonly phase?: string;
      readonly reference?: string;
      readonly created_at_ms?: number;
    }[];
  };
  const build = (value.items ?? [])
    .filter(
      (item) => item.phase === 'succeeded' && item.id !== undefined && item.reference !== undefined,
    )
    .sort((left, right) => (right.created_at_ms ?? 0) - (left.created_at_ms ?? 0))[0];
  if (build?.id === undefined || build.reference === undefined)
    throw new Error('The selected image has no successful immutable version.');
  return { id, version: build.id, reference: build.reference };
}
