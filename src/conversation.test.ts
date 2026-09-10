import { describe, expect, it } from 'vitest';
import { applyBlockEdit, buildConversationBlocks } from './conversation';
import { SILENCE_TEXT } from './types';
import type { TranscriptSegment } from './types';

function segment(id: string, start: number, end: number, text: string, speaker: number | null = 0): TranscriptSegment {
  return { id, meeting_id: 'm1', start_seconds: start, end_seconds: end, text, speaker };
}

describe('conversation blocks', () => {
  it('merges consecutive same-speaker segments and tracks every member id', () => {
    const blocks = buildConversationBlocks([
      segment('s1', 0, 30, 'Good morning everyone.'),
      segment('s2', 31, 60, 'Let us start with the budget.'),
      segment('s3', 90, 120, 'Moving on to hiring.', 1),
    ]);
    expect(blocks).toHaveLength(2);
    expect(blocks[0].segmentIds).toEqual(['s1', 's2']);
    expect(blocks[0].text).toBe('Good morning everyone. Let us start with the budget.');
    expect(blocks[1].segmentIds).toEqual(['s3']);
  });

  it('keeps silence placeholders and empty rows out of the reading view', () => {
    const blocks = buildConversationBlocks([
      segment('s0', 0, 5, SILENCE_TEXT, null),
      segment('s1', 6, 10, 'Real content.'),
      segment('s2', 11, 12, '   ', null),
    ]);
    expect(blocks.map((block) => block.key)).toEqual(['s1']);
  });

  it('keeps unknown-speaker segments apart unless they are close and short', () => {
    const far = buildConversationBlocks([segment('a', 0, 10, 'Hello?', null), segment('b', 60, 70, 'Hello?', null)]);
    expect(far).toHaveLength(2);
    const longTail = buildConversationBlocks([segment('a', 0, 10, 'x'.repeat(90), null), segment('b', 12, 20, 'Hello?', null)]);
    expect(longTail).toHaveLength(2);
    const close = buildConversationBlocks([segment('a', 0, 10, 'Hello?', null), segment('b', 14, 20, 'Hi!', null)]);
    expect(close).toHaveLength(1);
  });
});

describe('editing a merged conversation block', () => {
  it('rewrites the first segment with the edited text and drops the stale members', () => {
    const segments = [
      segment('s1', 0, 30, 'Good morning everyone.'),
      segment('s2', 31, 60, 'Let us start with the budget.'),
      segment('s3', 90, 120, 'Moving on to hiring.', 1),
    ];
    const block = buildConversationBlocks(segments)[0];
    const next = applyBlockEdit(segments, block, 'Edited opening line.');
    expect(next.map((item) => item.id)).toEqual(['s1', 's3']);
    expect(next[0].text).toBe('Edited opening line.');
    expect(next.find((item) => item.id === 's2')).toBeUndefined();
  });

  it('does not touch segments outside the edited block and keeps ordering', () => {
    const segments = [
      segment('a', 0, 10, 'First block.', 0),
      segment('b', 20, 30, 'Second block.', 1),
      segment('c', 31, 40, 'Second block tail.', 1),
    ];
    const blocks = buildConversationBlocks(segments);
    const next = applyBlockEdit(segments, blocks[1], 'Only this block changes.');
    expect(next.map((item) => item.id)).toEqual(['a', 'b']);
    expect(next[0].text).toBe('First block.');
    expect(next[1].text).toBe('Only this block changes.');
    expect(next.find((item) => item.id === 'c')).toBeUndefined();
  });
});
