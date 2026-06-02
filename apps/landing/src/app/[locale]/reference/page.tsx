import type { Metadata } from 'next';
import { getTranslations, setRequestLocale } from 'next-intl/server';
import { Github, Layers } from 'lucide-react';
import { Link } from '@/i18n/navigation';
import { routing } from '@/i18n/routing';
import { siteConfig } from '@/config/site';
import { SdkUsage } from '@/components/SdkUsage';
import { ModelsReference } from '@/components/ModelsReference';
import { ApiReference } from '@/components/ApiReference';
import styles from './reference.module.css';

type Props = { params: Promise<{ locale: string }> };

/** Pre-render the reference for every supported locale. */
export function generateStaticParams() {
  return routing.locales.map((locale) => ({ locale }));
}

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { locale } = await params;
  const t = await getTranslations({ locale, namespace: 'reference' });
  return { title: t('title'), alternates: { canonical: `/${locale}/reference` } };
}

/**
 * Single developer-reference page: SDK usage, the per-model capability table,
 * and the full REST API reference — everything on one page. Replaces the old
 * split between the `#sdks` homepage section and the standalone `/api` page.
 */
export default async function ReferencePage({ params }: Props) {
  const { locale } = await params;
  setRequestLocale(locale);
  const t = await getTranslations('reference');

  return (
    <>
      <header className={styles.bar}>
        <div className={`container ${styles.barInner}`}>
          <Link href="/" className={styles.brand} aria-label={siteConfig.name}>
            <span className={styles.mark} aria-hidden="true">
              <Layers size={18} />
            </span>
            <span>LiteGen</span>
          </Link>
          <nav className={styles.jump} aria-label={t('title')}>
            <a href="#sdks">{t('sdkSectionTitle')}</a>
            <a href="#models">{t('modelsHeading')}</a>
            <a href="#rest">{t('restHeading')}</a>
          </nav>
          <a
            className="btn btn-secondary"
            href={siteConfig.githubUrl}
            target="_blank"
            rel="noreferrer"
          >
            <Github size={16} />
            GitHub
          </a>
        </div>
      </header>
      <main>
        <SdkUsage />
        <ModelsReference />
        <section id="rest" className={styles.rest}>
          <div className="container">
            <h2 className={styles.restHeading}>{t('restHeading')}</h2>
            <p className={styles.restSub}>{t('restSub')}</p>
          </div>
          <ApiReference />
        </section>
      </main>
    </>
  );
}
