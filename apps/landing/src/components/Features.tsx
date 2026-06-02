import { useTranslations } from 'next-intl';
import {
  Activity,
  KeyRound,
  LayoutDashboard,
  LineChart,
  Plug,
  Route,
  Server,
  Zap,
  type LucideIcon,
} from 'lucide-react';
import { FEATURES } from '@/config/site';
import styles from './Features.module.css';

const ICONS: Record<string, LucideIcon> = {
  Plug,
  Route,
  Zap,
  LineChart,
  KeyRound,
  Activity,
  LayoutDashboard,
  Server,
};

export function Features() {
  const t = useTranslations('features');

  return (
    <section id="features" className={styles.section}>
      <div className="container">
        <header className={styles.head}>
          <h2 className={styles.heading}>{t('heading')}</h2>
          <p className={styles.sub}>{t('subheading')}</p>
        </header>

        <ul className={styles.grid}>
          {FEATURES.map(({ key, icon }) => {
            const Icon = ICONS[icon];
            return (
              <li key={key} className={styles.card}>
                <span className={styles.icon} aria-hidden="true">
                  <Icon size={20} />
                </span>
                <h3 className={styles.title}>{t(`items.${key}.title`)}</h3>
                <p className={styles.desc}>{t(`items.${key}.desc`)}</p>
              </li>
            );
          })}
        </ul>
      </div>
    </section>
  );
}
