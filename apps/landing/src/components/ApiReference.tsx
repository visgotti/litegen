'use client';

/**
 * In-page REST API reference, rendered natively in the site's dark theme.
 *
 * Replaces the previously-embedded Redoc widget (which shipped its own white,
 * three-pane app and clashed hard with the page). We read the OpenAPI spec at
 * build time and render it with the same visual language as {@link
 * ModelsReference}: tag groups, collapsible endpoint rows, and expandable
 * detail with parameters, request/response schemas, and a copyable curl call.
 */
import { useMemo, useState } from 'react';
import { useTranslations } from 'next-intl';
import { Check, Copy, Download } from 'lucide-react';
import spec from '../../public/openapi.json';
import { buildApiModel, type Endpoint, type Field, type OpenApiSpec } from '@/lib/openapi';
import styles from './ApiReference.module.css';

const MODEL = buildApiModel(spec as unknown as OpenApiSpec);
const METHOD_CLASS: Record<string, string> = {
  get: styles.get,
  post: styles.post,
  put: styles.put,
  patch: styles.patch,
  delete: styles.delete,
  head: styles.head,
  options: styles.options,
};

/** Small copy-to-clipboard button that flips to a check for a moment. */
function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className={styles.copy}
      aria-label={label}
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(text);
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1400);
        } catch {
          /* clipboard unavailable — no-op */
        }
      }}
    >
      {copied ? <Check size={14} /> : <Copy size={14} />}
    </button>
  );
}

/** Recursive schema field list — name · type · constraints · description. */
function FieldList({ fields, depth = 0 }: { fields: Field[]; depth?: number }) {
  return (
    <ul className={styles.fields} style={depth ? { marginLeft: 14 } : undefined}>
      {fields.map((f) => (
        <li key={f.name} className={styles.field}>
          <div className={styles.fieldHead}>
            <code className={styles.fieldName}>{f.name}</code>
            <span className={styles.fieldType}>{f.type}</span>
            {f.required ? (
              <span className={styles.req}>required</span>
            ) : (
              <span className={styles.opt}>optional</span>
            )}
            {f.default !== undefined && <span className={styles.fieldMeta}>default {f.default}</span>}
            {f.constraint && <span className={styles.fieldMeta}>{f.constraint}</span>}
          </div>
          {f.description && <p className={styles.fieldDesc}>{f.description}</p>}
          {f.enumValues && (
            <div className={styles.enums}>
              {f.enumValues.map((v) => (
                <code key={v} className={styles.enum}>
                  {v}
                </code>
              ))}
            </div>
          )}
          {f.fields && <FieldList fields={f.fields} depth={depth + 1} />}
        </li>
      ))}
    </ul>
  );
}

function statusClass(status: string): string {
  if (status.startsWith('2')) return styles.ok;
  if (status.startsWith('3')) return styles.redirect;
  if (status.startsWith('4')) return styles.warn;
  return styles.err;
}

function EndpointRow({ ep, t }: { ep: Endpoint; t: ReturnType<typeof useTranslations> }) {
  const [open, setOpen] = useState(false);
  return (
    <li id={ep.id} className={styles.row}>
      <button
        type="button"
        className={styles.rowHead}
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <span className={styles.chevron} aria-hidden="true">
          {open ? '▾' : '▸'}
        </span>
        <span className={`${styles.method} ${METHOD_CLASS[ep.method]}`}>{ep.method}</span>
        <code className={styles.path}>{ep.path}</code>
        <span className={styles.summary}>{ep.title}</span>
        {ep.deprecated && <span className={styles.deprecated}>{t('deprecated')}</span>}
      </button>

      {open && (
        <div className={styles.detail}>
          {ep.description && <p className={styles.endpointDesc}>{ep.description}</p>}

          {ep.params.length > 0 && (
            <section className={styles.block}>
              <h4 className={styles.blockTitle}>{t('parameters')}</h4>
              <ul className={styles.fields}>
                {ep.params.map((p) => (
                  <li key={`${p.in}-${p.name}`} className={styles.field}>
                    <div className={styles.fieldHead}>
                      <code className={styles.fieldName}>{p.name}</code>
                      <span className={styles.fieldType}>{p.type}</span>
                      <span className={styles.in}>{p.in}</span>
                      {p.required ? (
                        <span className={styles.req}>required</span>
                      ) : (
                        <span className={styles.opt}>optional</span>
                      )}
                      {p.constraint && <span className={styles.fieldMeta}>{p.constraint}</span>}
                    </div>
                    {p.description && <p className={styles.fieldDesc}>{p.description}</p>}
                  </li>
                ))}
              </ul>
            </section>
          )}

          {ep.bodyFields && (
            <section className={styles.block}>
              <h4 className={styles.blockTitle}>
                {t('requestBody')}
                {ep.bodySchemaName && <code className={styles.schemaName}>{ep.bodySchemaName}</code>}
                {!ep.bodyRequired && <span className={styles.opt}>{t('optionalBody')}</span>}
              </h4>
              <FieldList fields={ep.bodyFields} />
            </section>
          )}

          <section className={styles.block}>
            <h4 className={styles.blockTitle}>{t('example')}</h4>
            <div className={styles.code}>
              <CopyButton text={ep.curl} label={t('copy')} />
              <pre>
                <code>{ep.curl}</code>
              </pre>
            </div>
          </section>

          {ep.responses.length > 0 && (
            <section className={styles.block}>
              <h4 className={styles.blockTitle}>{t('responses')}</h4>
              <div className={styles.responses}>
                {ep.responses.map((r) => (
                  <div key={r.status} className={styles.response}>
                    <div className={styles.responseHead}>
                      <span className={`${styles.status} ${statusClass(r.status)}`}>{r.status}</span>
                      <span className={styles.responseDesc}>{r.description}</span>
                      {r.schemaName && <code className={styles.schemaName}>{r.schemaName}</code>}
                    </div>
                    {r.example && (
                      <div className={styles.code}>
                        <CopyButton text={r.example} label={t('copy')} />
                        <pre>
                          <code>{r.example}</code>
                        </pre>
                      </div>
                    )}
                  </div>
                ))}
              </div>
            </section>
          )}
        </div>
      )}
    </li>
  );
}

export function ApiReference() {
  const t = useTranslations('reference');
  const [query, setQuery] = useState('');
  const base = process.env.NEXT_PUBLIC_BASE_PATH ?? '';

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return MODEL.groups;
    return MODEL.groups
      .map((g) => ({
        ...g,
        endpoints: g.endpoints.filter(
          (e) =>
            e.path.toLowerCase().includes(q) ||
            e.title.toLowerCase().includes(q) ||
            e.method.includes(q) ||
            g.name.toLowerCase().includes(q),
        ),
      }))
      .filter((g) => g.endpoints.length > 0);
  }, [query]);

  return (
    <div className={`container ${styles.wrap}`}>
      <div className={styles.meta}>
        <div className={styles.metaText}>
          <span className={styles.version}>v{MODEL.version}</span>
          {MODEL.license && <span className={styles.license}>{MODEL.license}</span>}
          <span className={styles.endpointCount}>{t('endpointCount', { n: MODEL.count })}</span>
        </div>
        <a className={`btn btn-secondary ${styles.download}`} href={`${base}/openapi.json`} download>
          <Download size={15} />
          {t('downloadSpec')}
        </a>
      </div>

      <input
        type="search"
        className={styles.search}
        placeholder={t('searchEndpoints')}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        aria-label={t('searchEndpoints')}
      />

      {groups.length === 0 ? (
        <p className={styles.empty}>{t('noEndpoints')}</p>
      ) : (
        groups.map((g) => (
          <div key={g.name} className={styles.group}>
            <h3 className={styles.tag}>
              {g.name}
              <span className={styles.count}>{g.endpoints.length}</span>
              {g.description && <span className={styles.tagDesc}>{g.description}</span>}
            </h3>
            <ul className={styles.rows}>
              {g.endpoints.map((ep) => (
                <EndpointRow key={ep.id} ep={ep} t={t} />
              ))}
            </ul>
          </div>
        ))
      )}
    </div>
  );
}
