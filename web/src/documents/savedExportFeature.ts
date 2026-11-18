export function savedDocumentExportEnabled(): boolean {
  return import.meta.env.VITE_SAVED_DOCUMENT_EXPORT_ENABLED === 'true';
}
