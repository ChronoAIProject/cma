/* Start before the module graph so failed entry chunks are observable too.
 * Untrusted diagnostics only: never send messages, stacks, paths or identities. */
(function () {
  var sent = new Set();
  var kinds = new Set([
    'uncaught_error',
    'unhandled_rejection',
    'react_error',
    'resource_error',
    'chunk_error',
  ]);
  window.cmaReportFatal = function (kind) {
    if (!kinds.has(kind) || sent.has(kind)) return;
    sent.add(kind);
    try {
      void fetch('/api/v1/telemetry/browser', {
        method: 'POST',
        credentials: 'omit',
        mode: 'same-origin',
        keepalive: true,
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ kind: kind }),
      }).catch(function () {});
    } catch {
      // Diagnostics must never interrupt startup or recursively report themselves.
    }
  };
  window.addEventListener(
    'error',
    function (event) {
      var tag = event.target && event.target.tagName;
      if (tag === 'SCRIPT' || tag === 'LINK') {
        window.cmaReportFatal('resource_error');
      } else if (event instanceof ErrorEvent) {
        window.cmaReportFatal('uncaught_error');
      }
    },
    true,
  );
  window.addEventListener('unhandledrejection', function () {
    window.cmaReportFatal('unhandled_rejection');
  });
  window.addEventListener('vite:preloadError', function () {
    window.cmaReportFatal('chunk_error');
  });
})();
