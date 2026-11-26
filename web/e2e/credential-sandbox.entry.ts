import { createElement, StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { createClient } from '@cma/client';
import { createCreationClient } from '../src/api/creation';
import { createRestWebClient } from '../src/api/rest';
import { AuthContextProvider, useAuth } from '../src/auth/context';
import { createNyxidAuthProvider } from '../src/auth/nyxid';
import { NewSandboxCard } from '../src/create/NewSandboxCard';
import '../src/styles/tokens.css';
import '../src/styles/design-platform.css';
import '../src/styles/effects-typography.css';
import '../src/styles/scrollbar.css';
import '../src/styles/base.css';
import '../src/styles/components.css';

const sdk = createClient();
const client = createRestWebClient(sdk);
const catalog = createCreationClient(sdk);

// The portal no longer offers legacy Sandbox creation. Keep its retained
// component covered in a test-only entry, using the real HTTP/auth adapters.
function RetainedSandbox() {
  const { state } = useAuth();
  if (state.status !== 'authenticated') return null;
  return createElement(NewSandboxCard, {
    client,
    catalog,
    onClose: () => location.assign('/'),
    onCreated: (sandbox) => location.assign(`/sandboxes/${sandbox.id}`),
  });
}

createRoot(document.getElementById('root')!).render(
  createElement(
    StrictMode,
    null,
    createElement(AuthContextProvider, {
      provider: createNyxidAuthProvider(),
      children: createElement(RetainedSandbox),
    }),
  ),
);
