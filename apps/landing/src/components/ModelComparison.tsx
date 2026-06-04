'use client';

import { useState } from 'react';
import { useTranslations } from 'next-intl';
import { COMPARISONS } from '@/config/comparisons';
import styles from './ModelComparison.module.css';

// Mirror next.config's basePath so image URLs resolve under a subpath deploy.
const BASE = process.env.NEXT_PUBLIC_BASE_PATH?.trim().replace(/\/$/, '') || '';

/**
 * "Same prompt, every model" — pick one of the static evaluation prompts and see
 * each provider:model's output side by side. Entirely data-driven from
 * config/comparisons.ts; renders nothing until at least one prompt is authored.
 */
export function ModelComparison() {
  const t = useTranslations('comparison');
  const [activeId, setActiveId] = useState(COMPARISONS[0]?.id ?? '');

  // Hidden until at least one comparison exists (see config/comparisons.ts).
  if (COMPARISONS.length === 0) return null;

  const active = COMPARISONS.find((c) => c.id === activeId) ?? COMPARISONS[0];

  return (
    <section id="comparison" className={styles.section}>
      <div className="container">
        <header className={styles.head}>
          <span className="eyebrow">{t('eyebrow')}</span>
          <h2 className={styles.heading}>{t('heading')}</h2>
          <p className={styles.sub}>{t('subheading')}</p>
        </header>

        {COMPARISONS.length > 1 && (
          <div className={styles.pills} role="tablist" aria-label={t('selectLabel')}>
            {COMPARISONS.map((c) => {
              const isActive = c.id === active.id;
              return (
                <button
                  key={c.id}
                  type="button"
                  role="tab"
                  aria-selected={isActive}
                  className={`${styles.pill} ${isActive ? styles.pillActive : ''}`}
                  onClick={() => setActiveId(c.id)}
                >
                  {c.label ?? c.prompt}
                </button>
              );
            })}
          </div>
        )}

        <p className={styles.prompt}>“{active.prompt}”</p>

        <ul className={styles.grid}>
          {active.results.map((r) => (
            <li key={`${r.provider}:${r.model}:${r.image}`} className={styles.cell}>
              <div className={styles.frame}>
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img
                  src={`${BASE}/comparisons/${r.image}`}
                  alt={`${r.provider} ${r.model} — ${active.prompt}`}
                  loading="lazy"
                  decoding="async"
                />
              </div>
              <div className={styles.cap}>
                <span className={styles.prov}>{r.provider}</span>
                <span className={styles.sep}>:</span>
                <span className={styles.model}>{r.model}</span>
              </div>
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
