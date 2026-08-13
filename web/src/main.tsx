import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';

const container = document.getElementById('root');
if (!container) {
  throw new Error('root element missing');
}

/*
 * The concrete provider is chosen here and nowhere else. Replacing the mock
 * with the real NyxID adapter (#3) is a one-line change at this seam.
 */
const authProvider = createMockAuthProvider();

createRoot(container).render(
  <StrictMode>
    <AuthContextProvider provider={authProvider}>
      <App />
    </AuthContextProvider>
  </StrictMode>,
);
