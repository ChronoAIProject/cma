import { createContext, useContext } from 'react';

export const ValidationScope = createContext('');

export function useValidationPath(path?: string) {
  const scope = useContext(ValidationScope);
  if (!path) return undefined;
  return path.startsWith('/') ? path : scope ? `${scope}/${path}` : undefined;
}
