import { createContext, useContext, type ReactNode } from 'react';
import type { Fields } from './model';

interface HmacCredentials {
  value: Fields;
  create: boolean;
}

const Context = createContext<HmacCredentials | undefined>(undefined);

export function HmacCredentialsScope({
  value,
  create,
  children,
}: HmacCredentials & { children: ReactNode }) {
  return <Context value={{ value, create }}>{children}</Context>;
}

export const useHmacCredentials = () => useContext(Context);
