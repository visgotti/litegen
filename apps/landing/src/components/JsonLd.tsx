import { siteConfig } from '@/config/site';

/**
 * SoftwareApplication structured data (JSON-LD) for rich search results.
 * Rendered server-side into the static HTML.
 */
export function JsonLd() {
  const data = {
    '@context': 'https://schema.org',
    '@type': 'SoftwareApplication',
    name: siteConfig.name,
    applicationCategory: 'DeveloperApplication',
    operatingSystem: 'Linux, macOS, Docker',
    description:
      'Open-source, OpenAI-compatible proxy for AI image and video generation with routing, caching, and cost tracking.',
    url: siteConfig.url,
    sameAs: [siteConfig.githubUrl],
    offers: { '@type': 'Offer', price: '0', priceCurrency: 'USD' },
    license: siteConfig.licenseUrl,
  };

  return (
    <script
      type="application/ld+json"
      dangerouslySetInnerHTML={{ __html: JSON.stringify(data) }}
    />
  );
}
