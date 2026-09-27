/// Smallest context window Bea will record. Anything below this is a bad
/// catalog value or a typo, not a real window, so it is ignored and the
/// resolver's own minimum applies instead.
export const MIN_CONTEXT_TOKENS = 2_048;

/// Normalises a provider-reported context window into a value worth storing, or
/// null when there is nothing usable to record.
///
/// Locally discovered models (Ollama, LM Studio, llama.cpp) publish no window at
/// all, and a catalog can report nonsense. Dropping those here is what keeps the
/// Rust resolver's model-name table and per-kind defaults authoritative instead
/// of letting a bogus number decide how much context the prompt may use.
export const usableContextTokens = (value: number | null | undefined): number | null => {
  if (typeof value !== 'number' || !Number.isFinite(value)) return null;
  const tokens = Math.round(value);
  return tokens >= MIN_CONTEXT_TOKENS ? tokens : null;
};

/// Records the window a model was selected with, so the next chat question sizes
/// its prompt from the model instead of a fixed constant. Fire-and-forget: the
/// budget falls back to the model-name table if this cannot be written, so a
/// failed write must never interrupt the model switch itself.
export const rememberModelContext = async (
  model: string,
  contextTokens?: number | null,
): Promise<void> => {
  const tokens = usableContextTokens(contextTokens);
  if (!model || tokens === null) return;
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('set_model_context_window_command', {
      model,
      contextTokens: tokens,
      source: 'catalog',
    });
  } catch {
    // Ignored on purpose: budgeting stays on its fallbacks.
  }
};

/// Drops a recorded window so the model goes back to automatic detection.
/// Used when the user blanks the local-model context field: "blank" has to
/// remove the stored value, otherwise the field would keep claiming a window
/// the user no longer wants.
export const forgetModelContext = async (model: string): Promise<void> => {
  if (!model) return;
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('clear_model_context_window_command', { model });
  } catch {
    // Ignored on purpose: budgeting stays on its fallbacks.
  }
};
