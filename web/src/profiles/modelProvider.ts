export function isCodexConnection(service: string): boolean {
  return ['chrono-llm', 'chrono-llm-public', 'cma:codex-subscription'].includes(service);
}

export function codexProviderName(access: 'chronoai_managed' | 'own_credentials'): string {
  return access === 'own_credentials' ? 'Your ChatGPT subscription' : 'Chrono LLM (public)';
}
