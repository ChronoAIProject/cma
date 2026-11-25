import { useRef, useState } from 'react';

/** Only an explicitly selected, successfully listed path survives access rechecks. */
export class FileNavigationMemory {
  private directory?: string;

  read() {
    return this.directory;
  }

  remember(directory: string) {
    this.directory = directory;
  }
}

export function useFileNavigation(initialDirectory: string, memory?: FileNavigationMemory) {
  const remembered = memory?.read();
  const [state, setState] = useState({
    source: initialDirectory,
    initialRoot: remembered ?? initialDirectory,
    rootPath: remembered ?? initialDirectory,
    directory: remembered ?? initialDirectory,
    touched: remembered !== undefined,
  });
  if (state.source !== initialDirectory) {
    setState(
      state.touched
        ? { ...state, source: initialDirectory }
        : {
            ...state,
            source: initialDirectory,
            initialRoot: initialDirectory,
            rootPath: initialDirectory,
            directory: initialDirectory,
          },
    );
  }
  const revision = useRef(0);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const setDirectory = (directory: string) => {
    memory?.remember(directory);
    setState((current) => ({ ...current, directory, touched: true }));
  };
  const navigate = async (
    path: string,
    root: boolean,
    signal: AbortSignal | undefined,
    listed: boolean,
    load: (path: string, signal: AbortSignal) => Promise<boolean>,
  ) => {
    if (!signal || signal.aborted) return;
    const current = ++revision.current;
    setState((value) => ({ ...value, touched: true }));
    if (!listed && !(await load(path, signal))) return;
    if (signal.aborted || current !== revision.current) return;
    if (root) {
      memory?.remember(path);
      setState((value) => ({ ...value, rootPath: path, directory: path, touched: true }));
    } else {
      setDirectory(path);
      setExpanded((value) => {
        const next = new Set(value);
        if (next.has(path)) next.delete(path);
        else next.add(path);
        return next;
      });
    }
  };
  return {
    ...state,
    expanded,
    navigate,
    setDirectory,
    touch: () => setState((current) => ({ ...current, touched: true })),
  };
}
