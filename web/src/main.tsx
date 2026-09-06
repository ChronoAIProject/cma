import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createNyxidAuthProvider } from './auth/nyxid';
import { startAppearance } from './theme/appearance';
import { createClient } from '@chronoai/cma-sdk';
import { CmaClientProvider } from './api/context';
import { createRestWebClient } from './api/rest';
import { createProfileViewsClient } from './api/profiles';
import { createCreationClient } from './api/creation';

/*
 * Before the first render, so the page never paints in the wrong theme and
 * corrects itself a frame later.
 */
startAppearance();

const container = document.getElementById('root');
if (!container) {
  throw new Error('root element missing');
}

/*
 * The concrete provider is chosen here and nowhere else, which is what made
 * replacing the mock with the real adapter a one-line change. The mock remains
 * for tests.
 */
const authProvider = createNyxidAuthProvider();
const sdk = createClient();
const client = createRestWebClient(sdk);
const profiles = createProfileViewsClient(sdk);
const creation = createCreationClient(sdk);

createRoot(container).render(
  <StrictMode>
    <AuthContextProvider provider={authProvider}>
      <CmaClientProvider client={client} profiles={profiles} creation={creation}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>
  </StrictMode>,
);
