import { createContext, useContext } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { AnchorHTMLAttributes, ReactNode } from 'react';
import ChatTable from './ChatTable';
import CodeBlock from './CodeBlock';
import { tableFromNode, textFromNode } from './chatFormat';
import type { HastNode, TableExportHandler } from './chatFormat';

export type { ChatTableData, TableExportFormat, TableExportHandler } from './chatFormat';
export { copyText, tableToMarkdown, tableToTsv } from './chatFormat';

/// Tables are rendered by our own component so they can carry an export
/// toolbar; the handler arrives through context because react-markdown's
/// `components` map only forwards node props.
const TableExportContext = createContext<TableExportHandler | null>(null);
const useTableExport = () => useContext(TableExportContext);

const components = {
  a: ({ node: _node, ...props }: { node?: unknown } & AnchorHTMLAttributes<HTMLAnchorElement>) => (
    <a {...props} target="_blank" rel="noreferrer" />
  ),
  table: ({ node }: { node?: unknown }) => {
    const table = tableFromNode(node);
    // Hook first so the call order stays stable across renders even when the
    // node turns out not to be a table.
    const onExport = useTableExport();
    if (!table) return null;
    return <ChatTable table={table} onExport={onExport} />;
  },
  // Fenced blocks arrive as <pre><code>; CodeBlock renders its own <pre>, so
  // the wrapper is dropped rather than nested.
  pre: ({ children }: { children?: ReactNode }) => <>{children}</>,
  code: ({ node, className, children }: { node?: unknown; className?: string; children?: ReactNode }) => {
    const language = /language-(\w+)/.exec(className ?? '')?.[1];
    if (!language) return <code>{children}</code>;
    return <CodeBlock code={textFromNode(node as HastNode)} language={language} />;
  },
};

/// Renders assistant/user markdown with GFM support. Output is a React element
/// tree — no `dangerouslySetInnerHTML` — so it stays compatible with the app's
/// `script-src 'self'` CSP.
export default function Markdown({ text, onExportTable }: { text: string; onExportTable?: TableExportHandler }) {
  return (
    <TableExportContext.Provider value={onExportTable ?? null}>
      <div className="md">
        <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>{text}</ReactMarkdown>
      </div>
    </TableExportContext.Provider>
  );
}
