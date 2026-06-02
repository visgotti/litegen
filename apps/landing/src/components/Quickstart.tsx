import { useTranslations } from 'next-intl';
import { QUICKSTART_CODE } from '@/config/site';
import { CodeBlock } from './CodeBlock';
import styles from './Quickstart.module.css';

export function Quickstart() {
  const t = useTranslations('quickstart');

  return (
    <section id="quickstart" className={styles.section}>
      <div className="container">
        <header className={styles.head}>
          <h2 className={styles.heading}>{t('heading')}</h2>
          <p className={styles.sub}>{t('subheading')}</p>
        </header>

        <CodeBlock code={QUICKSTART_CODE} copyLabel={t('copy')} copiedLabel={t('copied')} lang="bash" />
      </div>
    </section>
  );
}
