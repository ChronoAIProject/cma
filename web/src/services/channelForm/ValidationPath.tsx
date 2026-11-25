import { fieldError } from './fieldErrors';
import { createContext, useContext } from 'react';

export const ValidationScope = createContext('');
export const ValidationMessageScope = createContext<string | undefined>(undefined);

export function useValidationPath(path?: string) {
  const scope = useContext(ValidationScope);
  if (!path) return undefined;
  return path.startsWith('/') ? path : scope ? `${scope}/${path}` : undefined;
}

export function useServerFieldError(path?: string) {
  const error = useContext(ValidationMessageScope);
  if (!error || !path) return undefined;
  const parsed = fieldError(error);
  return parsed.path === path ? parsed.message : undefined;
}
