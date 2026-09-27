/// Pure helpers shared by the chat markdown renderer, the table component and
/// their tests. Deliberately free of JSX so Vitest can exercise them without a
/// DOM — the React layer lives in `chatMarkdown.tsx`.

export type ChatTableData = { headers: string[]; rows: string[][] };
export type TableExportFormat = 'csv' | 'xlsx';
export type TableExportHandler = (table: ChatTableData, format: TableExportFormat) => void;

/// Minimal structural view of a hast node. Declared locally (rather than
/// importing `hast` types) so the table helpers stay independent of the
/// markdown pipeline.
export type HastNode = { type?: string; tagName?: string; value?: string; children?: HastNode[] };

const isElement = (node: HastNode | undefined, tagName: string): boolean =>
  node?.type === 'element' && node?.tagName === tagName;

const childrenOf = (node: HastNode | undefined, tagName: string): HastNode[] =>
  (node?.children ?? []).filter((child) => isElement(child, tagName));

/// Concatenates every text descendant of a hast node. Cells routinely hold
/// inline markup (`**bold**`, links, code spans) so a naive `.value` read
/// would silently drop content.
export function textFromNode(node: HastNode | undefined): string {
  if (!node) return '';
  if (node.type === 'text') return node.value ?? '';
  return (node.children ?? []).map((child) => textFromNode(child)).join('');
}

const cellsOf = (row: HastNode | undefined, tagName: 'th' | 'td'): string[] =>
  (row?.children ?? [])
    .filter((child) => isElement(child, tagName))
    .map((cell) => textFromNode(cell).trim());

/// Extracts `{ headers, rows }` from a rendered GFM table node. Returns
/// `undefined` for anything that is not a table, so callers can fall back to
/// rendering the default element.
export function tableFromNode(node: unknown): ChatTableData | undefined {
  if (!node || typeof node !== 'object') return undefined;
  const table = node as HastNode;
  if (!isElement(table, 'table')) return undefined;
  const headRows = childrenOf(childrenOf(table, 'thead')[0], 'tr');
  const headers = headRows.length ? cellsOf(headRows[0], 'th') : [];
  const bodyRows = childrenOf(table, 'tbody').flatMap((body) =>
    childrenOf(body, 'tr').map((row) => cellsOf(row, 'td')),
  );
  if (!headers.length && !bodyRows.length) return undefined;
  return { headers, rows: bodyRows };
}

/// Tab-separated so the clipboard lands directly in Excel / Google Sheets.
/// Tabs and newlines inside a cell would break the column alignment, so they
/// collapse to spaces.
export function tableToTsv(table: ChatTableData): string {
  const flatten = (cell: string) => cell.replace(/[\t\r\n]+/g, ' ').trim();
  const headerLine = table.headers.length ? [table.headers.map(flatten).join('\t')] : [];
  return [...headerLine, ...table.rows.map((row) => row.map(flatten).join('\t'))].join('\n');
}

const escapePipe = (cell: string) => cell.replace(/\|/g, '\\|').replace(/[\r\n]+/g, '<br>');

/// RFC 4180 quoting for the browser-preview CSV path. A cell starting with
/// `=`, `+`, `-` or `@` is prefixed with a quote so a spreadsheet treats it as
/// text instead of a formula.
export function tableToCsv(table: ChatTableData): string {
  const quote = (cell: string) => {
    const guarded = /^[=+\-@]/.test(cell) ? `'${cell}` : cell;
    return /[",\r\n]/.test(guarded) ? `"${guarded.replace(/"/g, '""')}"` : guarded;
  };
  const width = Math.max(table.headers.length, ...table.rows.map((row) => row.length), 1);
  const header = table.headers.length
    ? [Array.from({ length: width }, (_, index) => quote(table.headers[index] ?? '')).join(',')]
    : [];
  const body = table.rows.map((row) => Array.from({ length: width }, (_, index) => quote(row[index] ?? '')).join(','));
  return [...header, ...body].join('\r\n');
}

/// Rebuilds the GitHub-flavored pipe table so it round-trips back into a
/// markdown editor or a viewer. Headerless tables get generated `Column N`
/// headers so the result is still a valid GFM table.
export function tableToMarkdown(table: ChatTableData): string {
  const headers = table.headers.length
    ? table.headers
    : table.rows[0]?.map((_, index) => `Column ${index + 1}`) ?? [];
  const body = table.headers.length ? table.rows : table.rows.slice(1);
  const head = `| ${headers.map(escapePipe).join(' | ')} |`;
  const rule = `| ${headers.map(() => '---').join(' | ')} |`;
  const rows = body.map((row) => `| ${headers.map((_, index) => escapePipe(row[index] ?? '')).join(' | ')} |`);
  return [head, rule, ...rows].join('\n');
}

/// Writes to the clipboard, resolving `false` instead of throwing so callers
/// can surface a toast when the WebView denies clipboard access.
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
