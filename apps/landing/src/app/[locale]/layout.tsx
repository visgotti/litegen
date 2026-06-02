import type { Metadata } from 'next';
import { NextIntlClientProvider } from 'next-intl';
import { getMessages, getTranslations, setRequestLocale } from 'next-intl/server';
import { notFound } from 'next/navigation';
import { routing, type Locale } from '@/i18n/routing';
import { siteConfig } from '@/config/site';
import '@/styles/globals.css';

type LocaleParams = { params: Promise<{ locale: string }> };

/** Pre-render one static page per supported locale. */
export function generateStaticParams() {
  return routing.locales.map((locale) => ({ locale }));
}

export async function generateMetadata({ params }: LocaleParams): Promise<Metadata> {
  const { locale } = await params;
  const t = await getTranslations({ locale, namespace: 'meta' });

  // hreflang alternates so search engines serve the right language.
  const languages = Object.fromEntries(routing.locales.map((l) => [l, `/${l}`]));

  return {
    metadataBase: new URL(siteConfig.url),
    title: { default: t('title'), template: `%s · ${siteConfig.name}` },
    description: t('description'),
    applicationName: siteConfig.name,
    keywords: [
      'AI image generation',
      'AI video generation',
      'OpenAI compatible',
      'image generation proxy',
      'LiteLLM for images',
      'DALL-E',
      'Stable Diffusion',
      'Replicate',
      'Fal',
      'self-hosted AI gateway',
    ],
    authors: [{ name: 'LiteGen' }],
    alternates: {
      canonical: `/${locale}`,
      languages,
    },
    openGraph: {
      type: 'website',
      siteName: siteConfig.name,
      title: t('title'),
      description: t('description'),
      url: `${siteConfig.url}/${locale}`,
      locale,
    },
    twitter: {
      card: 'summary_large_image',
      title: t('title'),
      description: t('description'),
    },
    robots: {
      index: true,
      follow: true,
      googleBot: { index: true, follow: true, 'max-image-preview': 'large' },
    },
  };
}

export default async function LocaleLayout({
  children,
  params,
}: LocaleParams & { children: React.ReactNode }) {
  const { locale } = await params;
  if (!routing.locales.includes(locale as Locale)) {
    notFound();
  }

  // Enables static rendering for this locale.
  setRequestLocale(locale);
  const messages = await getMessages();

  return (
    <html lang={locale}>
      <body>
        <NextIntlClientProvider messages={messages}>{children}</NextIntlClientProvider>
      </body>
    </html>
  );
}
