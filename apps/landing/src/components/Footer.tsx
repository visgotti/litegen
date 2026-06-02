import { useTranslations } from 'next-intl';
import { Layers } from 'lucide-react';
import { siteConfig } from '@/config/site';
import styles from './Footer.module.css';

export function Footer() {
  const t = useTranslations('footer');

  return (
    <footer className={styles.footer}>
      <div className={`container ${styles.inner}`}>
        <div className={styles.brandCol}>
          <span className={styles.brand}>
            <span className={styles.mark} aria-hidden="true">
              <Layers size={16} />
            </span>
            LiteGen
          </span>
          <p className={styles.tagline}>{t('tagline')}</p>
        </div>

        <nav className={styles.links} aria-label="Footer">
          <a href={siteConfig.githubUrl} target="_blank" rel="noreferrer">
            {t('github')}
          </a>
          <a href={siteConfig.docsUrl} target="_blank" rel="noreferrer">
            {t('docs')}
          </a>
          <a href={siteConfig.licenseUrl} target="_blank" rel="noreferrer">
            {t('license')}
          </a>
        </nav>
      </div>
      <div className={`container ${styles.legal}`}>
        <span>© {siteConfig.name}</span>
        <span>{t('rights')}</span>
      </div>
    </footer>
  );
}
