import { setRequestLocale } from 'next-intl/server';
import { JsonLd } from '@/components/JsonLd';
import { Nav } from '@/components/Nav';
import { Hero } from '@/components/Hero';
import { InfraFlow } from '@/components/InfraFlow';
import { Features } from '@/components/Features';
import { Providers } from '@/components/Providers';
import { Quickstart } from '@/components/Quickstart';
import { Footer } from '@/components/Footer';

type Props = { params: Promise<{ locale: string }> };

export default async function Home({ params }: Props) {
  const { locale } = await params;
  setRequestLocale(locale);

  return (
    <>
      <JsonLd />
      <Nav />
      <main>
        <Hero />
        <InfraFlow />
        <Features />
        <Providers />
        <Quickstart />
      </main>
      <Footer />
    </>
  );
}
