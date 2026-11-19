import { useState } from 'react';

export function useFileNavigation(initialDirectory: string) {
  const [state, setState] = useState({
    source: initialDirectory,
    initialRoot: initialDirectory,
    rootPath: initialDirectory,
    directory: initialDirectory,
    touched: false,
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
  return {
    ...state,
    touch: () => setState((current) => ({ ...current, touched: true })),
    setRootPath: (rootPath: string) =>
      setState((current) => ({ ...current, rootPath, directory: rootPath, touched: true })),
    setDirectory: (directory: string) =>
      setState((current) => ({ ...current, directory, touched: true })),
  };
}
