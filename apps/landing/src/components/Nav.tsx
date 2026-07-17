import { useTranslations } from 'next-intl';
import { Github, LayoutDashboard } from 'lucide-react';
import { siteConfig } from '@/config/site';
import { Link } from '@/i18n/navigation';
import { LangSwitcher } from './LangSwitcher';
import styles from './Nav.module.css';

// Mirror next.config's basePath so the logo resolves under a subpath deploy.
const BASE = process.env.NEXT_PUBLIC_BASE_PATH?.trim().replace(/\/$/, '') || '';

export function Nav() {
  const t = useTranslations('nav');

  return (
    <header className={styles.header}>
      <div className={`container ${styles.inner}`}>
        <a href="#top" className={styles.brand} aria-label={siteConfig.name}>
          <img
            className={styles.mark}
            src={`${BASE}/logos/litegen-logo.png`}
            alt=""
            aria-hidden="true"
            decoding="async"
          />
          <span className={styles.wordmark}>LiteGen</span>
        </a>

        <nav className={styles.links} aria-label="Primary">
          <a href="#how-it-works">{t('howItWorks')}</a>
          <a href="#features">{t('features')}</a>
          <a href="#providers">{t('providers')}</a>
          <a href="#quickstart">{t('quickstart')}</a>
          <Link href="/reference">{t('reference')}</Link>
        </nav>

        <div className={styles.actions}>
          <LangSwitcher />
          <a className={`btn btn-primary ${styles.dashboard}`} href={siteConfig.appUrl}>
            <LayoutDashboard size={16} />
            {t('dashboard')}
          </a>
          <a
            className={`btn btn-secondary ${styles.github}`}
            href={siteConfig.githubUrl}
            target="_blank"
            rel="noreferrer"
          >
            <Github size={16} />
            {t('github')}
          </a>
        </div>
      </div>
    </header>
  );
}
