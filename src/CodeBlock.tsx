import { useEffect, useState } from 'react';
import { copyText } from './chatFormat';
import Icon from './Icon';

/// Fenced code block with a language tag and a copy button. The trailing
/// newline react-markdown keeps on the last line is dropped so copied code does
/// not carry a phantom blank line.
const trimTrailingNewline = (code: string) => code.replace(/\n$/, '');

export default function CodeBlock({ code, language }: { code: string; language: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1600);
    return () => clearTimeout(timer);
  }, [copied]);

  return <figure className="md-code">
    <figcaption className="md-code-bar">
      <span className="md-code-lang">{language}</span>
      <button type="button" className="md-code-copy" onClick={() => { void copyText(trimTrailingNewline(code)).then(setCopied); }} title="Copy code">
        <Icon name={copied ? 'check' : 'copy'} size={12} />{copied ? 'Copied' : 'Copy'}
      </button>
    </figcaption>
    <pre><code>{trimTrailingNewline(code)}</code></pre>
  </figure>;
}
