export type SlashParse = { action: string | null; arg: string };

const ACTIONS = ['clarify', 'context', 'correction', 'custom'] as const;

export function parseSlashCommand(input: string): SlashParse {
  const trimmed = input.trim();
  if (!trimmed.startsWith('/')) return { action: null, arg: trimmed };
  const [head, ...rest] = trimmed.slice(1).split(/\s+/);
  if (head === 'correction') {
    // Mass-correction syntax: /correction FIND=>REPLACE
    return { action: 'correction', arg: rest.join(' ') };
  }
  if ((ACTIONS as readonly string[]).includes(head)) {
    return { action: head, arg: rest.join(' ') };
  }
  return { action: `unknown:/${head}`, arg: rest.join(' ') };
}

export const SLASH_HELP = [
  '/clarify <note> — add a clarification Bea should honor in minutes and answers',
  '/context <fact> — add background context the meeting transcript lacks',
  '/correction FIND=>REPLACE — fix a name/term across the whole transcript',
  '/custom — open the custom minutes-format editor (markdown)',
];

export type ClarificationQuestion = { question: string; options: string[] };

/// Parses the JSON the model returns for suggested clarifications. Returns []
/// for any malformed output so the minutes flow never blocks on a bad reply.
export function parseClarificationSuggestions(raw: string): ClarificationQuestion[] {
  try {
    const parsed: unknown = JSON.parse(raw);
    const questions = (parsed as { questions?: unknown })?.questions;
    if (!Array.isArray(questions)) return [];
    return questions
      .filter((item): item is { question: string; options?: string[] } =>
        Boolean(item) && typeof (item as { question?: unknown }).question === 'string')
      .map((item) => ({
        question: item.question,
        options: Array.isArray(item.options) ? item.options.filter((o): o is string => typeof o === 'string').slice(0, 4) : [],
      }))
      .slice(0, 5);
  } catch {
    return [];
  }
}
