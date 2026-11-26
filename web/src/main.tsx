import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createNyxidAuthProvider } from './auth/nyxid';
import { currentTransportUser } from './auth/retryMetadata';
import { replicaRecoveryEnabled } from './agent/transport/recoveryConfig';
import { readRetryDelay } from './agent/transport/readRetry';
import { startAppearance } from './theme/appearance';
import { startTextSize } from './theme/textSize';
import { createClient } from '@cma/client';
import { CmaClientProvider } from './api/context';
import { createRestWebClient } from './api/rest';
import { createProfileViewsClient } from './api/profiles';
import { createCreationClient } from './api/creation';
import { createOperatorClient } from './operator/client';
import { createTriggerPublisherClient } from './trigger-publisher/client';
import { createPublicClient } from './public/client';
import { prepareHistoryDocument } from './routing/historyDocument';

/*
 * Before the first render, so the page never paints in the wrong theme and
 * corrects itself a frame later.
 */
startAppearance();
startTextSize();

const container = document.getElementById('root');
if (!container) {
  throw new Error('root element missing');
}

/*
 * The concrete provider is chosen here and nowhere else, which is what made
 * replacing the mock with the real adapter a one-line change. The mock remains
 * for tests.
 */
if (prepareHistoryDocument()) {
  const identityProvider = createNyxidAuthProvider();
  const sdk = createClient();
  const client = createRestWebClient(sdk, {
    storage: sessionStorage,
    recovery: { enabled: replicaRecoveryEnabled, delay: readRetryDelay },
    currentUser: currentTransportUser(identityProvider),
  });
  const profiles = createProfileViewsClient(sdk);
  const creation = createCreationClient(sdk);
  const operator = createOperatorClient(sdk);
  const publisher = createTriggerPublisherClient(sdk);
  const publicClient = createPublicClient(sdk);

  createRoot(container, {
    onUncaughtError(error) {
      window.cmaReportFatal?.('react_error');
      console.error(error);
    },
    onCaughtError(error) {
      window.cmaReportFatal?.('react_error');
      console.error(error);
    },
  }).render(
    <StrictMode>
      <AuthContextProvider provider={identityProvider}>
        <CmaClientProvider
          client={client}
          profiles={profiles}
          creation={creation}
          operator={operator}
          publisher={publisher}
          publicClient={publicClient}
        >
          <App />
        </CmaClientProvider>
      </AuthContextProvider>
    </StrictMode>,
  );
}
