'use client';

import { useEffect } from 'react';
import { Link, useRouter } from '@/i18n/navigation';

/**
 * The old `/api` reference page was folded into `/reference`. This client stub
 * redirects on load (works under static export, where server redirects can't
 * run) and shows a plain link as the no-JS fallback.
 */
export function RedirectToReference() {
  const router = useRouter();
  useEffect(() => {
    router.replace('/reference#rest');
  }, [router]);

  return (
    <main style={{ padding: '64px 24px', textAlign: 'center' }}>
      <p>
        The API reference moved to{' '}
        <Link href="/reference#rest">/reference</Link>.
      </p>
    </main>
  );
}
