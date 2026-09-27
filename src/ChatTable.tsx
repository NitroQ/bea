import { useEffect, useState } from 'react';
import { copyText, tableToTsv, tableToMarkdown } from './chatFormat';
import type { ChatTableData, TableExportHandler } from './chatFormat';
import Icon from './Icon';

const FLASH_MS = 1600;

/// GFM table rendered by us rather than by react-markdown so it can carry the
/// per-table export toolbar. Copy writes TSV, which pastes cleanly into Excel
/// and Google Sheets; the file exports hand the same data to the host so the
/// desktop build can write a real .csv/.xlsx.
export default function ChatTable({ table, onExport }: { table: ChatTableData; onExport?: TableExportHandler | null }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), FLASH_MS);
    return () => clearTimeout(timer);
  }, [copied]);

  // Headerless tables still need one <th> per column to keep the grid aligned.
  const columns = Math.max(table.headers.length, ...table.rows.map((row) => row.length), 1);
  const header = (index: number) => table.headers[index] ?? (table.headers.length ? '' : `Column ${index + 1}`);

  return <figure className="md-table">
    <div className="md-table-scroll">
      <table>
        {table.headers.length > 0 && <thead><tr>
          {Array.from({ length: columns }, (_, index) => <th key={index} scope="col">{header(index)}</th>)}
        </tr></thead>}
        <tbody>
          {table.rows.map((row, rowIndex) => <tr key={rowIndex}>
            {Array.from({ length: columns }, (_, index) => <td key={index}>{row[index] ?? ''}</td>)}
          </tr>)}
        </tbody>
      </table>
    </div>
    <figcaption className="md-table-bar">
      <span className="md-table-meta"><Icon name="table" size={12} />{table.rows.length} {table.rows.length === 1 ? 'row' : 'rows'}</span>
      <div className="md-table-actions">
        <button type="button" onClick={() => { void copyText(tableToTsv(table)).then(setCopied); }} title="Copy table (tab-separated)">
          <Icon name={copied ? 'check' : 'copy'} size={12} />{copied ? 'Copied' : 'Copy'}
        </button>
        <button type="button" onClick={() => { void copyText(tableToMarkdown(table)).then(setCopied); }} title="Copy table as Markdown">Markdown</button>
        {onExport && <button type="button" onClick={() => onExport(table, 'csv')} title="Download as CSV">
          <Icon name="download" size={12} />CSV
        </button>}
        {onExport && <button type="button" onClick={() => onExport(table, 'xlsx')} title="Download as Excel workbook">
          <Icon name="download" size={12} />XLSX
        </button>}
      </div>
    </figcaption>
  </figure>;
}
