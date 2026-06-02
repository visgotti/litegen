import type { Metadata } from 'next';
import { setRequestLocale } from 'next-intl/server';
import { routing } from '@/i18n/routing';
import { RedirectToReference } from './RedirectToReference';

type Props = { params: Promise<{ locale: string }> };

/** Pre-render the redirect stub for every supported locale. */
export function generateStaticParams() {
  return routing.locales.map((locale) => ({ locale }));
}

export const metadata: Metadata = { robots: { index: false, follow: false } };

export default async function ApiRedirectPage({ params }: Props) {
  const { locale } = await params;
  setRequestLocale(locale);
  return <RedirectToReference />;
}
