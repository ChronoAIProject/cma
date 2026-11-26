import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from 'react';

export interface ConfigurationSection {
  readonly id: string;
  readonly title: string;
  readonly element: HTMLElement;
}
export const ConfigurationSections = createContext<
  ((section: ConfigurationSection) => () => void) | undefined
>(undefined);
export const ConfigurationDepth = createContext(0);
export const OpenProducerConfiguration = createContext<(() => void) | undefined>(undefined);

export function useConfigurationSections() {
  const [sections, setSections] = useState<readonly ConfigurationSection[]>([]);
  const register = useCallback((section: ConfigurationSection) => {
    setSections((current) => [...current.filter((item) => item.id !== section.id), section]);
    return () => setSections((current) => current.filter((item) => item.id !== section.id));
  }, []);
  const ordered = useMemo(
    () =>
      [...sections].sort((a, b) =>
        a.element.compareDocumentPosition(b.element) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1,
      ),
    [sections],
  );
  return { sections: ordered, register };
}

export function useConfigurationSection(title: string, navigation?: boolean) {
  const id = useId();
  const ref = useRef<HTMLFieldSetElement>(null);
  const register = useContext(ConfigurationSections);
  const depth = useContext(ConfigurationDepth);
  const navigable = navigation ?? depth === 0;
  useEffect(() => {
    if (navigable && ref.current && register) return register({ id, title, element: ref.current });
  }, [id, title, navigable, register]);
  return { id, ref, depth: navigation === false ? depth : depth + 1, navigable };
}

export function scrollToConfiguration(id: string, animate = true, focus = true) {
  const target = document.getElementById(id);
  target?.scrollIntoView?.({
    block: 'start',
    behavior:
      animate && !window.matchMedia('(prefers-reduced-motion: reduce)').matches
        ? 'smooth'
        : 'instant',
  });
  if (focus) target?.focus({ preventScroll: true });
}
