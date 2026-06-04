import { useTranslations } from 'next-intl';
import { PROVIDERS } from '@/config/site';
import styles from './Providers.module.css';

// Mirror next.config's basePath so logo URLs resolve under a subpath deploy
// (e.g. GitHub Pages) as well as at the domain root (Cloudflare).
const BASE = process.env.NEXT_PUBLIC_BASE_PATH?.trim().replace(/\/$/, '') || '';

export function Providers() {
  const t = useTranslations('providers');

  return (
    <section id="providers" className={styles.section}>
      <div className="container">
        <header className={styles.head}>
          <h2 className={styles.heading}>{t('heading')}</h2>
          <p className={styles.sub}>{t('subheading')}</p>
        </header>

        <ul className={styles.grid}>
          {PROVIDERS.map((p) => (
            <li key={p.name} className={styles.chip}>
              {p.logo ? (
                <span className={styles.logo} aria-hidden="true">
                  {/* eslint-disable-next-line @next/next/no-img-element */}
                  <img src={`${BASE}/logos/${p.logo}`} alt="" loading="lazy" decoding="async" />
                </span>
              ) : (
                <span className={styles.dot} aria-hidden="true">
                  {p.name.charAt(0)}
                </span>
              )}
              <span className={styles.name}>{p.name}</span>
              <span className={styles.tags}>
                {p.image && <span className={styles.tag}>{t('image')}</span>}
                {p.video && (
                  <span className={`${styles.tag} ${styles.tagVideo}`}>{t('video')}</span>
                )}
              </span>
            </li>
          ))}
          <li className={`${styles.chip} ${styles.more}`}>{t('more')}</li>
        </ul>
      </div>
    </section>
  );
}
