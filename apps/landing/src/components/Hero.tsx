import { useTranslations } from 'next-intl';
import { ArrowRight, Github, LayoutDashboard, Sparkles } from 'lucide-react';
import { siteConfig } from '@/config/site';
import styles from './Hero.module.css';

export function Hero() {
  const t = useTranslations('hero');

  return (
    <section id="top" className={styles.hero}>
      <div className={`container ${styles.grid}`}>
        <div className={styles.copy}>
          <span className="eyebrow">
            <Sparkles size={14} />
            {t('badge')}
          </span>
          <h1 className={styles.title}>{t('title')}</h1>
          <p className={styles.subtitle}>{t('subtitle')}</p>

          <div className={styles.ctas}>
            <a className="btn btn-primary" href={siteConfig.appUrl}>
              <LayoutDashboard size={16} />
              {t('ctaApp')}
              <ArrowRight size={16} />
            </a>
            <a className="btn btn-secondary" href={siteConfig.docsUrl} target="_blank" rel="noreferrer">
              {t('ctaPrimary')}
            </a>
            <a className="btn btn-secondary" href={siteConfig.githubUrl} target="_blank" rel="noreferrer">
              <Github size={16} />
              {t('ctaSecondary')}
            </a>
          </div>
        </div>

        <figure className={styles.terminal} aria-label="Example LiteGen request and response">
          <div className={styles.chrome}>
            <span /> <span /> <span />
          </div>
          <pre className={styles.code}>
            <code>
              <span className={styles.prompt}>$</span> curl litegen/v1/images/generations \{'\n'}
              {'  '}-d {"'"}
              {'{'}
              <span className={styles.key}>&quot;model&quot;</span>:
              <span className={styles.str}>&quot;openai/dall-e-3&quot;</span>,{'\n'}
              {'        '}
              <span className={styles.key}>&quot;prompt&quot;</span>:
              <span className={styles.str}>&quot;a red panda coding&quot;</span>
              {'}'}
              {"'"}
              {'\n\n'}
              {'{'}
              {'\n'}
              {'  '}
              <span className={styles.key}>&quot;provider&quot;</span>:{' '}
              <span className={styles.str}>&quot;openai&quot;</span>,{'\n'}
              {'  '}
              <span className={styles.key}>&quot;cost_usd&quot;</span>:{' '}
              <span className={styles.num}>0.04</span>,{'\n'}
              {'  '}
              <span className={styles.key}>&quot;data&quot;</span>: [{'{'}{' '}
              <span className={styles.key}>&quot;url&quot;</span>:{' '}
              <span className={styles.str}>&quot;https://…&quot;</span> {'}'}]{'\n'}
              {'}'}
            </code>
          </pre>
          <figcaption className={styles.caption}>{t('codeCaption')}</figcaption>
        </figure>
      </div>
    </section>
  );
}
