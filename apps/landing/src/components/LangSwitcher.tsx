'use client';

import { useLocale } from 'next-intl';
import { Globe } from 'lucide-react';
import { usePathname, useRouter } from '@/i18n/navigation';
import { routing } from '@/i18n/routing';
import styles from './LangSwitcher.module.css';

const LABELS: Record<string, string> = { en: 'EN', es: 'ES' };

/** Switches locale while preserving the current path (i18n routing in action). */
export function LangSwitcher() {
  const locale = useLocale();
  const pathname = usePathname();
  const router = useRouter();

  return (
    <label className={styles.wrap}>
      <Globe size={15} aria-hidden="true" />
      <span className="sr-only">Language</span>
      <select
        className={styles.select}
        value={locale}
        onChange={(e) => router.replace(pathname, { locale: e.target.value })}
        aria-label="Language"
      >
        {routing.locales.map((l) => (
          <option key={l} value={l}>
            {LABELS[l] ?? l.toUpperCase()}
          </option>
        ))}
      </select>
    </label>
  );
}
