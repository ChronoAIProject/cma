import { useState } from 'react';

function storageKey(viewerId: string): string {
  return `cma:tree:${viewerId}:collapsed`;
}

function restored(viewerId: string): Set<string> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(storageKey(viewerId)) ?? '[]');
    return new Set(
      Array.isArray(parsed)
        ? parsed.filter((value): value is string => typeof value === 'string')
        : [],
    );
  } catch {
    return new Set();
  }
}

export function useTreeCollapse(viewerId: string) {
  const [collapsed, setCollapsed] = useState(() => restored(viewerId));
  const toggle = (node: string) => {
    setCollapsed((current) => {
      const next = new Set(current);
      if (next.has(node)) next.delete(node);
      else next.add(node);
      localStorage.setItem(storageKey(viewerId), JSON.stringify([...next]));
      return next;
    });
  };
  return { collapsed, toggle };
}
