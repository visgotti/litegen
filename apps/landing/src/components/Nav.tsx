import { useTranslations } from 'next-intl';
import { Github, Layers } from 'lucide-react';
import { siteConfig } from '@/config/site';
import { Link } from '@/i18n/navigation';
import { LangSwitcher } from './LangSwitcher';
import styles from './Nav.module.css';

export function Nav() {
  const t = useTranslations('nav');

  return (
    <header className={styles.header}>
      <div className={`container ${styles.inner}`}>
        <a href="#top" className={styles.brand} aria-label={siteConfig.name}>
          <span className={styles.mark} aria-hidden="true">
            <Layers size={18} />
          </span>
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
