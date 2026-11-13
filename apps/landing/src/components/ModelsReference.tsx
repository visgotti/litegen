'use client';

/**
 * Browsable per-model capability reference. Renders the build-generated
 * `MODELS` catalog (from models/*.yaml) grouped by provider, with a search box
 * and expandable rows. A row's head says what the model does and its exact
 * price per unit; its details give the full spec — the vendor's lifecycle,
 * sizes (listed, or freeform bounds), aspect ratios (and the frame images
 * that set the shape instead), reference-image roles,
 * prompt limits, parameters with their limits, and the price with its rules.
 * The wording lives in src/lib/{price,model}-format.mjs, tested in scripts/.
 */
import { useMemo, useState } from 'react';
import { useLocale, useTranslations } from 'next-intl';
import { MODELS, type ModelEntry } from '@/config/models.generated';
import { formatUsd, priceDefaultText, priceText, ruleText } from '@/lib/price-format.mjs';
import { aspectRatioFramesText, lifecycleText, paramLimitsText, presetText, sizeRangeText } from '@/lib/model-format.mjs';
import styles from './ModelsReference.module.css';

/** The prompt line of a model that takes none, by what it works on instead. */
const NO_PROMPT: Record<NonNullable<ModelEntry['worksOn']> | 'image', string> = {
  image: 'noPrompt',
  sourceModel: 'noPromptSourceModel',
  sourceVideo: 'noPromptSourceVideo',
  segmentation: 'noPromptSegmentation',
};

export function ModelsReference() {
  const t = useTranslations('reference');
  const locale = useLocale();
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
            m.tags.some((tag) => tag.toLowerCase().includes(q)) ||
            m.task.toLowerCase().includes(q),
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
        <p className={styles.note}>{t('priceNote')}</p>

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
                          {m.modalities.map((mod) => (
                            <span key={mod} className={styles.badge}>
                              {t(`modality.${mod}`)}
                            </span>
                          ))}
                          <span className={`${styles.badge} ${styles[m.output]}`}>{m.output}</span>
                          {m.task !== 'generate' && (
                            <span className={`${styles.badge} ${styles.task}`} data-task={m.task}>
                              {m.task}
                            </span>
                          )}
                          {/* Deprecated from deprecated_at on, or once a shutdown date is
                              announced (schema.rs status_on); the dates are in the details. */}
                          {(m.lifecycle?.deprecatedAt || m.lifecycle?.retiresAt) && (
                            <span className={`${styles.badge} ${styles.deprecated}`}>{t('deprecated')}</span>
                          )}
                        </span>
                        <span className={styles.metaRow}>
                          {m.maxRefImages > 0 && (
                            <span className={styles.refs}>
                              {t('refsShort', { n: m.maxRefImages })}
                            </span>
                          )}
                          {m.pricing && (
                            <span className={styles.price}>
                              {priceText(m.pricing, t, locale)}
                              {m.pricing.rules.length > 0 && (
                                <span className={styles.varies}> · {t('priceVaries')}</span>
                              )}
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
                            {m.lifecycle && (
                              <>
                                <dt>{t('lifecycle')}</dt>
                                <dd>{lifecycleText(m.lifecycle, t)}</dd>
                              </>
                            )}
                            {(m.sizes.length > 0 || m.sizeRange) && (
                              <>
                                <dt title={m.sizeLabel}>{t('sizes')}</dt>
                                <dd>
                                  {m.sizeRange ? sizeRangeText(m.sizeRange, t, locale) : m.sizes.join(' · ')}
                                  {m.sizeDescription && <p className={styles.paramDesc}>{m.sizeDescription}</p>}
                                </dd>
                              </>
                            )}
                            {m.aspectRatios.length > 0 && (
                              <>
                                <dt title={m.aspectRatioLabel}>{t('aspectRatios')}</dt>
                                <dd>
                                  {m.aspectRatios.join(' · ')}
                                  {/* A frame image sets the shape instead (validator.rs check_frame_shape). */}
                                  {m.aspectRatioFrames && (
                                    <p className={styles.paramDesc}>{aspectRatioFramesText(m.aspectRatioFrames, t, locale)}</p>
                                  )}
                                  {m.aspectRatioDescription && (
                                    <p className={styles.paramDesc}>{m.aspectRatioDescription}</p>
                                  )}
                                </dd>
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
                              {/* max_length 0 is the catalog's "takes no prompt at all",
                                  not a zero-character limit: the model works from its input
                                  image, its source mesh or clip, or the prompts it segments by. */}
                              {m.promptLimits.maxLength === 0 ? (
                                t(NO_PROMPT[m.worksOn ?? 'image'])
                              ) : (
                                <>
                                  {m.promptLimits.required ? t('required') : t('optional')}
                                  {m.promptLimits.maxLength
                                    ? ` · ${t('maxChars', { n: m.promptLimits.maxLength })}`
                                    : ''}
                                </>
                              )}
                            </dd>
                            {m.params.length > 0 && (
                              <>
                                <dt>{t('params')}</dt>
                                <dd>
                                  <ul className={styles.params}>
                                    {m.params.map((p) => {
                                      const limits = paramLimitsText(p, t, locale);
                                      return (
                                        <li key={p.name}>
                                          <code title={p.label}>{p.name}</code>{' '}
                                          <span className={styles.kind}>{p.kind}</span>
                                          {p.enum && <> {p.enum.join(' | ')}</>}
                                          {/* A number that takes a list takes only those values:
                                              the validator refuses anything between them. */}
                                          {p.values && <> {p.values.join(' | ')}</>}
                                          {p.name === 'output_formats' && (
                                            <span className={styles.default}> · {t('formatsNote')}</span>
                                          )}
                                          {!p.values && (p.min !== undefined || p.max !== undefined) && (
                                            <>
                                              {' '}
                                              [{p.min ?? '−∞'}, {p.max ?? '∞'}]
                                            </>
                                          )}
                                          {p.default !== undefined && (
                                            <span className={styles.default}>
                                              {' '}
                                              {t('defaultEq', {
                                                v: Array.isArray(p.default) ? p.default.join(', ') : String(p.default),
                                              })}
                                            </span>
                                          )}
                                          {limits.length > 0 && (
                                            <span className={styles.default}> · {limits.join(' · ')}</span>
                                          )}
                                          {p.description && <p className={styles.paramDesc}>{p.description}</p>}
                                          {p.presets && p.presets.length > 0 && (
                                            <>
                                              <p className={styles.paramDesc}>{t('presets', { n: p.presets.length })}</p>
                                              <ul className={styles.presets}>
                                                {p.presets.map((preset) => (
                                                  <li key={preset.id}>{presetText(preset)}</li>
                                                ))}
                                              </ul>
                                            </>
                                          )}
                                        </li>
                                      );
                                    })}
                                  </ul>
                                </dd>
                              </>
                            )}
                            {m.pricing && (
                              <>
                                <dt>{t('price')}</dt>
                                <dd>
                                  <p className={styles.priceLine}>{priceDefaultText(m, t, locale)}</p>
                                  {m.pricing.rules.length > 0 && (
                                    <>
                                      <p className={styles.priceLine}>
                                        {t('priceRulesIntro', { zero: formatUsd(0, locale) })}
                                      </p>
                                      <ol className={styles.priceRules}>
                                        {m.pricing.rules.map((rule, i) => (
                                          <li key={i}>{ruleText(rule, t, locale)}</li>
                                        ))}
                                      </ol>
                                    </>
                                  )}
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
