import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';

type ImageManagerAvailability = 'loading' | 'legacy' | 'retired';

const ImageManagerAvailabilityContext = createContext<ImageManagerAvailability>('legacy');

export function useImageManagerAvailability(): ImageManagerAvailability {
  return useContext(ImageManagerAvailabilityContext);
}

export function ImageManagerRetirement({ children }: { readonly children: ReactNode }) {
  const [availability, setAvailability] = useState<ImageManagerAvailability>('loading');

  useEffect(() => {
    const lifetime = new AbortController();
    const signal = AbortSignal.any([lifetime.signal, AbortSignal.timeout(3000)]);
    void fetch('/api/v1/ui/image-manager-retirement', { cache: 'no-store', signal })
      .then(async (response) =>
        response.ok ? ((await response.json()) as unknown) === true : false,
      )
      .then((retired) => {
        if (!lifetime.signal.aborted) setAvailability(retired ? 'retired' : 'legacy');
      })
      .catch(() => {
        // Retained API releases do not advertise retirement.
        if (!lifetime.signal.aborted) setAvailability('legacy');
      });
    return () => lifetime.abort();
  }, []);

  return (
    <ImageManagerAvailabilityContext.Provider value={availability}>
      {children}
    </ImageManagerAvailabilityContext.Provider>
  );
}
