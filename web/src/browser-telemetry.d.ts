interface Window {
  cmaReportFatal?: (
    kind:
      'uncaught_error' | 'unhandled_rejection' | 'react_error' | 'resource_error' | 'chunk_error',
  ) => void;
}
