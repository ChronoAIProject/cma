/** Narrow untrusted event payloads before projecting conversation state. */
export function object(value: unknown): Readonly<Record<string, unknown>> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Readonly<Record<string, unknown>>)
    : undefined;
}

export function text(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

export function number(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

export function upsert<T extends { readonly id: string }>(
  items: readonly T[],
  item: T,
): readonly T[] {
  const at = items.findIndex(({ id }) => id === item.id);
  if (at < 0) return [...items, item];
  return items.map((current, index) => (index === at ? item : current));
}
