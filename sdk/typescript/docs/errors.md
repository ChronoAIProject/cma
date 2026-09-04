# Errors

<!-- nav: 4 Errors -->

All SDK failures derive from `CmaError`. `ApiError` represents a structured
non-success response, `ProtocolError` rejects an invalid response boundary, and
`TransportError` covers failures before an HTTP answer is available.

```ts
import { ApiError, CmaError, createClient } from '@chronoai/cma-sdk';

try {
  await createClient({ baseUrl: 'https://cma.example.com' }).readConformance();
} catch (error) {
  if (error instanceof ApiError) {
    console.error(error.status, error.code, error.requestId);
  } else if (error instanceof CmaError) {
    console.error(error.name, error.message);
  } else {
    throw error;
  }
}
```

`TimeoutError` and `RequestAborted` distinguish deadlines from deliberate
caller cancellation. Credential, request-id, and response-observer callbacks
also have dedicated errors.
