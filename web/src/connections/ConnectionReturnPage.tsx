import { useState } from 'react';
import { clearSameTabReturn, readSameTabReturn } from './sameTabReturn';

export function ConnectionReturnPage() {
  const [returnPath] = useState(readSameTabReturn);
  return (
    <main className="app-error">
      <h1 className="app-error__title">Returning to CMA</h1>
      {returnPath === undefined ? (
        <>
          <p className="app-error__next">
            CMA is checking your connection. Your next sign-in will open here, or this window will
            close when all services are ready. If nothing happens, return to the CMA window to
            continue.
          </p>
          <button type="button" onClick={() => window.close()}>
            Close this window
          </button>
        </>
      ) : (
        <>
          <p className="app-error__next">
            Return to your original request. CMA will verify your connection before continuing.
          </p>
          <a href={returnPath} onClick={clearSameTabReturn}>
            Return to CMA
          </a>
        </>
      )}
    </main>
  );
}
