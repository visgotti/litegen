'use client';

/**
 * Browsable per-model capability reference. Renders the build-generated
 * `MODELS` catalog (from models/*.yaml) grouped by provider, with a search box
 * and expandable rows that reveal each model's full spec — sizes, aspect
 * ratios, reference-image roles, prompt limits, parameters, and pricing.
 */
import { useMemo, useState } from 'react';
import { useTranslations } from 'next-intl';
import { MODELS, type ModelEntry } from '@/config/models.generated';
import styles from './ModelsReference.module.css';

function modalities(c: ModelEntry['capabilities']): string[] {
  const out: string[] = [];
  if (c.textToImage) out.push('text→image');
  if (c.imageToImage) out.push('image→image');
  if (c.inpainting) out.push('inpainting');
  if (c.textToVideo) out.push('text→video');
  if (c.imageToVideo) out.push('image→video');
  return out;
}

export function ModelsReference() {
  const t = useTranslations('reference');
  const [query, setQuery] = useState('');
  const [open, setOpen] = useState<Set<string>>(new Set());

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = q
      ? MODELS.filter(
          (m) =>
            m.id.toLowerCase().includes(q) ||
            m.provider.toLowerCase().includes(q) ||
            m.displayName.toLowerCase().includes(q) ||
            m.tags.some((tag) => tag.toLowerCase().includes(q)),
        )
      : MODELS;
    const byProvider = new Map<string, ModelEntry[]>();
    for (const m of filtered) {
      const arr = byProvider.get(m.provider) ?? [];
      arr.push(m);
      byProvider.set(m.provider, arr);
    }
    return [...byProvider.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [query]);

  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <section id="models" className={styles.section}>
      <div className="container">
        <header className={styles.head}>
          <h2 className={styles.heading}>{t('modelsHeading')}</h2>
          <p className={styles.sub}>{t('modelsSub')}</p>
        </header>

        <input
          type="search"
          className={styles.search}
          placeholder={t('searchPlaceholder')}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          aria-label={t('searchPlaceholder')}
        />

        {groups.length === 0 ? (
          <p className={styles.empty}>{t('noResults')}</p>
        ) : (
          groups.map(([provider, models]) => (
            <div key={provider} className={styles.group}>
              <h3 className={styles.provider}>
                {provider}
                <span className={styles.count}>{models.length}</span>
              </h3>
              <ul className={styles.rows}>
                {models.map((m) => {
                  const isOpen = open.has(m.id);
                  return (
                    <li key={m.id} className={styles.row}>
                      <button
                        type="button"
                        className={styles.rowHead}
                        aria-expanded={isOpen}
                        onClick={() => toggle(m.id)}
                      >
                        <span className={styles.chevron} aria-hidden="true">
                          {isOpen ? '▾' : '▸'}
                        </span>
                        <code className={styles.id}>{m.id}</code>
                        <span className={styles.badges}>
                          {modalities(m.capabilities).map((mod) => (
                            <span key={mod} className={styles.badge}>
                              {mod}
                            </span>
                          ))}
                          <span
                            className={`${styles.badge} ${
                              m.output === 'video' ? styles.video : styles.image
                            }`}
                          >
                            {m.output}
                          </span>
                        </span>
                        <span className={styles.metaRow}>
                          {m.maxRefImages > 0 && (
                            <span className={styles.refs}>
                              {t('refsShort', { n: m.maxRefImages })}
                            </span>
                          )}
                          {m.pricing && (
                            <span className={styles.price}>
                              ${m.pricing.baseCostUsd.toFixed(2)}
                            </span>
                          )}
                        </span>
                      </button>

                      {isOpen && (
                        <div className={styles.detail}>
                          {m.description && <p className={styles.desc}>{m.description}</p>}
                          <dl className={styles.specs}>
                            {m.referenceUrl && (
                              <>
                                <dt>{t('docsLabel')}</dt>
                                <dd>
                                  <a
                                    className={styles.docLink}
                                    href={m.referenceUrl}
                                    target="_blank"
                                    rel="noreferrer"
                                  >
                                    {t('viewDocs')}
                                  </a>
                                </dd>
                              </>
                            )}
                            {m.sizes.length > 0 && (
                              <>
                                <dt>{t('sizes')}</dt>
                                <dd>{m.sizes.join(' · ')}</dd>
                              </>
                            )}
                            {m.aspectRatios.length > 0 && (
                              <>
                                <dt>{t('aspectRatios')}</dt>
                                <dd>{m.aspectRatios.join(' · ')}</dd>
                              </>
                            )}
                            <dt>{t('referenceImages')}</dt>
                            <dd>
                              {m.maxRefImages === 0
                                ? t('none')
                                : t('refsRoles', {
                                    n: m.maxRefImages,
                                    roles: m.refRoles.map((r) => r.name).join(', ') || '—',
                                  })}
                            </dd>
                            <dt>{t('prompt')}</dt>
                            <dd>
                              {m.promptLimits.required ? t('required') : t('optional')}
                              {m.promptLimits.maxLength
                                ? ` · ${t('maxChars', { n: m.promptLimits.maxLength })}`
                                : ''}
                            </dd>
                            {m.params.length > 0 && (
                              <>
                                <dt>{t('params')}</dt>
                                <dd>
                                  <ul className={styles.params}>
                                    {m.params.map((p) => (
                                      <li key={p.name}>
                                        <code>{p.name}</code>{' '}
                                        <span className={styles.kind}>{p.kind}</span>
                                        {p.enum && <> {p.enum.join(' | ')}</>}
                                        {(p.min !== undefined || p.max !== undefined) && (
                                          <>
                                            {' '}
                                            [{p.min ?? '−∞'}, {p.max ?? '∞'}]
                                          </>
                                        )}
                                        {p.default !== undefined && (
                                          <span className={styles.default}>
                                            {' '}
                                            {t('defaultEq', { v: String(p.default) })}
                                          </span>
                                        )}
                                      </li>
                                    ))}
                                  </ul>
                                </dd>
                              </>
                            )}
                            {m.tags.length > 0 && (
                              <>
                                <dt>{t('tags')}</dt>
                                <dd className={styles.tags}>
                                  {m.tags.map((tag) => (
                                    <span key={tag} className={styles.tag}>
                                      {tag}
                                    </span>
                                  ))}
                                </dd>
                              </>
                            )}
                          </dl>
                        </div>
                      )}
                    </li>
                  );
                })}
              </ul>
            </div>
          ))
        )}
      </div>
    </section>
  );
}
