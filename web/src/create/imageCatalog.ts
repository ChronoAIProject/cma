export interface AgentImageChoice {
  readonly id: string;
  readonly name: string;
  readonly description?: string;
  readonly owner?: string;
  readonly retired?: boolean;
  readonly revision?: number;
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
  const response = await fetch('/api/image-manager/v1/images', {
    credentials: 'same-origin',
    cache: 'no-store',
    signal,
  });
  if (!response.ok) throw new Error(`Image Manager is unavailable (${String(response.status)}).`);
  const value = (await response.json()) as ImagePage;
  return Array.isArray(value.items) ? value.items : [];
}

