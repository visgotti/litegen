'use client';

import { useMemo, useState } from 'react';
import { Check, Copy } from 'lucide-react';
import hljs from 'highlight.js/lib/core';
import typescript from 'highlight.js/lib/languages/typescript';
import python from 'highlight.js/lib/languages/python';
import bash from 'highlight.js/lib/languages/bash';
import styles from './CodeBlock.module.css';

// Register only the languages our snippets use, to keep the bundle small.
hljs.registerLanguage('typescript', typescript);
hljs.registerLanguage('python', python);
hljs.registerLanguage('bash', bash);

type Props = {
  code: string;
  copyLabel: string;
  copiedLabel: string;
  /** Highlight language, e.g. "typescript" | "python" | "bash". */
  lang?: string;
};

export function CodeBlock({ code, copyLabel, copiedLabel, lang }: Props) {
  const [copied, setCopied] = useState(false);

  // Highlight deterministically so SSR and client hydration produce identical
  // markup. Falls back to plain (escaped) text for unknown/missing languages.
  const html = useMemo(() => {
    if (lang && hljs.getLanguage(lang)) {
      return hljs.highlight(code, { language: lang, ignoreIllegals: true }).value;
    }
    return null;
  }, [code, lang]);

  async function copy() {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    } catch {
      // Clipboard unavailable (e.g. insecure context) — fail silently.
    }
  }

  return (
    <div className={styles.wrap}>
      <button type="button" className={styles.copy} onClick={copy} aria-label={copyLabel}>
        {copied ? <Check size={15} /> : <Copy size={15} />}
        <span>{copied ? copiedLabel : copyLabel}</span>
      </button>
      <pre className={styles.pre}>
        {html !== null ? (
          <code className="hljs" dangerouslySetInnerHTML={{ __html: html }} />
        ) : (
          <code className="hljs">{code}</code>
        )}
      </pre>
    </div>
  );
}
