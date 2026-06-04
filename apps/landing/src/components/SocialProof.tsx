import { useTranslations } from 'next-intl';
import {
  Ghost,
  Building2,
  Unplug,
  MailWarning,
  Trophy,
  UserX,
  Bot,
  BadgeCheck,
  SquareDashed,
  type LucideIcon,
} from 'lucide-react';
import styles from './SocialProof.module.css';

// Deadpan "logo wall": every "customer" is a confession that nobody uses LiteGen.
const LOGOS: { name: string; Icon: LucideIcon }[] = [
  { name: 'No one', Icon: Ghost },
  { name: 'Not a real company', Icon: Building2 },
  { name: 'Not really using it', Icon: Unplug },
  { name: 'Got spam mail from them once', Icon: MailWarning },
  { name: 'pls be first', Icon: Trophy },
  { name: 'Literally nobody', Icon: UserX },
  { name: 'A bot, probably', Icon: Bot },
  { name: 'trust me dawg', Icon: BadgeCheck },
  { name: 'Your logo here', Icon: SquareDashed },
];

export function SocialProof() {
  const t = useTranslations('socialProof');

  return (
    <section id="trusted-by" className={styles.section}>
      <div className="container">
        <header className={styles.head}>
          <span className="eyebrow">{t('eyebrow')}</span>
          <h2 className={styles.heading}>{t('heading')}</h2>
          <p className={styles.sub}>{t('subheading')}</p>
        </header>
      </div>

      {/* Full-bleed scrolling marquee. The track holds the set twice so the
          translateX(-50%) animation loops with no visible seam. */}
      <div className={styles.marquee}>
        <ul className={styles.track}>
          {LOGOS.map(({ name, Icon }) => (
            <li key={name} className={styles.item}>
              <Icon className={styles.icon} size={20} aria-hidden="true" />
              <span>{name}</span>
            </li>
          ))}
          {LOGOS.map(({ name, Icon }) => (
            <li key={`dup-${name}`} className={`${styles.item} ${styles.dup}`} aria-hidden="true">
              <Icon className={styles.icon} size={20} aria-hidden="true" />
              <span>{name}</span>
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
