'use client';

/**
 * SDK usage section — a small TypeScript/Python toggle showing how to install a
 * first-party SDK and generate an image or video. The code snippets live in
 * `config/site.ts` (untranslated); only the surrounding labels are localized.
 */
import { useState } from 'react';
import { useTranslations } from 'next-intl';
import { CodeBlock } from './CodeBlock';
import {
  SDK_TS_INSTALL,
  SDK_TS_CODE,
  SDK_PY_INSTALL,
  SDK_PY_CODE,
} from '@/config/site';
import styles from './SdkUsage.module.css';

type Lang = 'ts' | 'py';

export function SdkUsage() {
  const t = useTranslations('sdks');
  const [lang, setLang] = useState<Lang>('ts');

  const install = lang === 'ts' ? SDK_TS_INSTALL : SDK_PY_INSTALL;
  const code = lang === 'ts' ? SDK_TS_CODE : SDK_PY_CODE;
  const copy = t('copy');
  const copied = t('copied');

  return (
    <section id="sdks" className={styles.section}>
      <div className="container">
        <header className={styles.head}>
          <h2 className={styles.heading}>{t('heading')}</h2>
          <p className={styles.sub}>{t('subheading')}</p>
        </header>

        <div className={styles.tabs} role="tablist" aria-label={t('tablistLabel')}>
          <button
            type="button"
            role="tab"
            aria-selected={lang === 'ts'}
            className={`${styles.tab} ${lang === 'ts' ? styles.active : ''}`}
            onClick={() => setLang('ts')}
          >
            {t('typescript')}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={lang === 'py'}
            className={`${styles.tab} ${lang === 'py' ? styles.active : ''}`}
            onClick={() => setLang('py')}
          >
            {t('python')}
          </button>
        </div>

        <p className={styles.step}>{t('installLabel')}</p>
        <CodeBlock code={install} copyLabel={copy} copiedLabel={copied} lang="bash" />

        <p className={styles.step}>{t('useLabel')}</p>
        <CodeBlock
          code={code}
          copyLabel={copy}
          copiedLabel={copied}
          lang={lang === 'ts' ? 'typescript' : 'python'}
        />
      </div>
    </section>
  );
}
