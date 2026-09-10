import { isSilenceSegment } from './types';
import type { TranscriptSegment } from './types';

/// Conversation view model: consecutive segments from the same speaker merge
/// into one flowing block (per-speaker paragraphs) instead of one row per
/// minute. Silence placeholders and empty rows are dropped from the reading
/// view — their total is still summarized in the toolbar.
///
/// `segmentIds` records every transcript segment that fed the block so an
/// edit can replace the whole block (update the first segment, delete the
/// rest) instead of duplicating stale text back into the transcript.
export type ConversationBlock = { key: string; speaker: number | null; startSeconds: number; endSeconds: number; text: string; segmentIds: string[] };

export function buildConversationBlocks(segments: TranscriptSegment[]): ConversationBlock[] {
  const blocks: ConversationBlock[] = [];
  for (const segment of segments) {
    if (isSilenceSegment(segment) || !segment.text.trim()) continue;
    const speaker = segment.speaker ?? null;
    const previous = blocks[blocks.length - 1];
    // Only merge adjacent segments if they have a known matching speaker (speaker !== null)
    // and are within a short pause. If speaker is null / unknown, keep segments distinct (or merge only within 5s).
    const sameKnownSpeaker = speaker !== null && previous && previous.speaker === speaker;
    const smallGap = previous ? segment.start_seconds - previous.endSeconds <= 5 : false;
    const canMerge = previous && (sameKnownSpeaker ? segment.start_seconds - previous.endSeconds <= 30 : smallGap && previous.speaker === null && previous.text.length < 80);
    if (canMerge && previous) {
      previous.text = `${previous.text} ${segment.text.trim()}`;
      previous.endSeconds = segment.end_seconds;
      previous.segmentIds.push(segment.id);
    } else {
      blocks.push({ key: segment.id, speaker, startSeconds: segment.start_seconds, endSeconds: segment.end_seconds, text: segment.text.trim(), segmentIds: [segment.id] });
    }
  }
  return blocks;
}

/// Applies an edit to a merged conversation block: the first member segment
/// receives the new text and every later member is removed, so the transcript
/// no longer keeps the pre-merge wording around it.
export function applyBlockEdit(segments: TranscriptSegment[], block: ConversationBlock, text: string): TranscriptSegment[] {
  const removed = new Set(block.segmentIds.slice(1));
  return segments
    .map((segment) => (segment.id === block.key ? { ...segment, text } : segment))
    .filter((segment) => !removed.has(segment.id));
}
