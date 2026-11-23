export function topographicEnabled(): boolean {
  return import.meta.env.VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED === 'true';
}
