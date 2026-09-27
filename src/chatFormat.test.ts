import { describe, expect, it } from 'vitest';
import { tableFromNode, tableToMarkdown, tableToTsv, textFromNode } from './chatFormat';
import type { HastNode } from './chatFormat';

const text = (value: string): HastNode => ({ type: 'text', value });
const cell = (value: string): HastNode => ({ type: 'element', tagName: 'td', children: [text(value)] });

const table = (children: HastNode[]): HastNode => ({ type: 'element', tagName: 'table', children });

describe('textFromNode', () => {
  it('returns empty string for missing nodes', () => {
    expect(textFromNode(undefined)).toBe('');
  });
  it('reads a bare text node', () => {
    expect(textFromNode(text('hello'))).toBe('hello');
  });
  it('concatenates nested inline markup instead of dropping it', () => {
    const node: HastNode = {
      type: 'element',
      tagName: 'strong',
      children: [text('Budget '), { type: 'element', tagName: 'em', children: [text('plan')] }],
    };
    expect(textFromNode(node)).toBe('Budget plan');
  });
});

describe('tableFromNode', () => {
  it('returns undefined for non-table nodes', () => {
    expect(tableFromNode(undefined)).toBeUndefined();
    expect(tableFromNode('table')).toBeUndefined();
    expect(tableFromNode({ type: 'element', tagName: 'p', children: [] })).toBeUndefined();
  });
  it('returns undefined for an empty table shell', () => {
    expect(tableFromNode(table([{ type: 'element', tagName: 'tbody', children: [] }]))).toBeUndefined();
  });
  it('separates headers from body rows and trims cell whitespace', () => {
    const node = table([
      {
        type: 'element',
        tagName: 'thead',
        children: [{ type: 'element', tagName: 'tr', children: [{ type: 'element', tagName: 'th', children: [text(' Owner ')] }, { type: 'element', tagName: 'th', children: [text('Due')] }] }],
      },
      {
        type: 'element',
        tagName: 'tbody',
        children: [
          { type: 'element', tagName: 'tr', children: [cell('Ana'), cell('2026-01-04')] },
          { type: 'element', tagName: 'tr', children: [cell('Bo'), cell('2026-02-01')] },
        ],
      },
    ]);
    expect(tableFromNode(node)).toEqual({
      headers: ['Owner', 'Due'],
      rows: [['Ana', '2026-01-04'], ['Bo', '2026-02-01']],
    });
  });
  it('keeps headerless tables usable for export', () => {
    const node = table([{ type: 'element', tagName: 'tbody', children: [{ type: 'element', tagName: 'tr', children: [cell('a'), cell('b')] }] }]);
    expect(tableFromNode(node)).toEqual({ headers: [], rows: [['a', 'b']] });
  });
  it('flattens inline markup inside cells', () => {
    const node = table([{
      type: 'element',
      tagName: 'tbody',
      children: [{ type: 'element', tagName: 'tr', children: [{ type: 'element', tagName: 'td', children: [text('see '), { type: 'element', tagName: 'a', children: [text('ledger')] }] }] }],
    }]);
    expect(tableFromNode(node)?.rows).toEqual([['see ledger']]);
  });
});

describe('tableToTsv', () => {
  it('emits a header line then one line per row', () => {
    const data = { headers: ['Owner', 'Due'], rows: [['Ana', '2026-01-04'], ['Bo', '2026-02-01']] };
    expect(tableToTsv(data)).toBe('Owner\tDue\nAna\t2026-01-04\nBo\t2026-02-01');
  });
  it('collapses tabs and newlines so columns stay aligned', () => {
    const data = { headers: ['Note'], rows: [['line one\nline two\tend']] };
    expect(tableToTsv(data)).toBe('Note\nline one line two end');
  });
  it('omits the header line when the table has none', () => {
    expect(tableToTsv({ headers: [], rows: [['a', 'b']] })).toBe('a\tb');
  });
});

describe('tableToMarkdown', () => {
  it('round-trips a headered table into a GFM pipe table', () => {
    const data = { headers: ['Owner', 'Due'], rows: [['Ana', '2026-01-04'], ['Bo', '2026-02-01']] };
    expect(tableToMarkdown(data)).toBe('| Owner | Due |\n| --- | --- |\n| Ana | 2026-01-04 |\n| Bo | 2026-02-01 |');
  });
  it('escapes pipes and flattens newlines so the table does not break', () => {
    const data = { headers: ['A|B'], rows: [['line one\nline two']] };
    expect(tableToMarkdown(data)).toBe('| A\\|B |\n| --- |\n| line one<br>line two |');
  });
  it('generates headers and promotes the first row for a headerless table', () => {
    expect(tableToMarkdown({ headers: [], rows: [['a', 'b'], ['c', 'd']] }))
      .toBe('| Column 1 | Column 2 |\n| --- | --- |\n| c | d |');
  });
  it('pads short rows so every line has the same column count', () => {
    expect(tableToMarkdown({ headers: ['A', 'B'], rows: [['only']] })).toBe('| A | B |\n| --- | --- |\n| only |  |');
  });
});
