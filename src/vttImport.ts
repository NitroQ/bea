export type VttCueSummary = { start_seconds: number; end_seconds: number; speaker: string | null; text: string };
export function vttSummary(cues: VttCueSummary[]): { cueCount: number; speakerNames: string[] } {
  const names: string[] = [];
  for (const cue of cues) {
    if (cue.speaker && !names.includes(cue.speaker)) names.push(cue.speaker);
  }
  return { cueCount: cues.length, speakerNames: names };
}
