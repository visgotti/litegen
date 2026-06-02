import { INestApplication, ValidationPipe, VersioningType } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import request from 'supertest';
import { AppModule } from '../src/app.module';
import { ScrapeService } from '../src/modules/scraping/scrape.service';

/**
 * The "god" e2e spec: a single suite that boots the whole application against a
 * real Postgres and exercises EVERY endpoint and every meaningful branch —
 * public reads + filters, the OAuth token flow (success, bad grant, bad creds,
 * scope subset, invalid scope), route protection (401/403), and every admin
 * write (price upsert create+update+tier, provider patch, scrape trigger for
 * scraped/stub providers, client creation) plus 404/validation error paths.
 *
 * Ordering matters: read-only assertions run before any mutation so seeded
 * state is observed first; scrape triggers and upserts (which mutate freshness
 * and prices) run last.
 */
describe('price-api god e2e', () => {
  let app: INestApplication;
  let http: ReturnType<typeof request>;
  let adminToken: string;

  const creds = (extra: Record<string, unknown> = {}) => ({
    grant_type: 'client_credentials',
    client_id: 'test-admin',
    client_secret: 'test-admin-secret',
    ...extra,
  });

  beforeAll(async () => {
    const moduleRef = await Test.createTestingModule({ imports: [AppModule] }).compile();
    app = moduleRef.createNestApplication();
    app.enableVersioning({ type: VersioningType.URI });
    app.useGlobalPipes(
      new ValidationPipe({ whitelist: true, forbidNonWhitelisted: true, transform: true }),
    );
    await app.init(); // runs SeedService via onApplicationBootstrap
    http = request(app.getHttpServer());

    const res = await http.post('/oauth/token').send(creds()).expect(200);
    adminToken = res.body.access_token;
  });

  afterAll(async () => {
    await app?.close();
  });

  const auth = () => ({ Authorization: `Bearer ${adminToken}` });

  // ---------------------------------------------------------------- health
  describe('health', () => {
    it('liveness', async () => {
      const res = await http.get('/health').expect(200);
      expect(res.body.status).toBe('ok');
    });
    it('readiness checks the database', async () => {
      const res = await http.get('/health/ready').expect(200);
      expect(res.body.details.database.status).toBe('up');
    });
  });

  // ------------------------------------------------------------- providers
  describe('providers', () => {
    it('lists all 7 providers with mode + scraper flag', async () => {
      const res = await http.get('/v1/providers').expect(200);
      expect(res.body).toHaveLength(7);
      const openai = res.body.find((p: any) => p.id === 'openai');
      expect(openai.mode).toBe('scraped');
      expect(openai.scraperImplemented).toBe(true);
      expect(openai.modelCount).toBe(3);
      const luma = res.body.find((p: any) => p.id === 'luma');
      expect(luma.mode).toBe('manual');
      expect(luma.scraperImplemented).toBe(false);
    });
    it('returns a provider with its models and prices', async () => {
      const res = await http.get('/v1/providers/openai').expect(200);
      expect(res.body.id).toBe('openai');
      expect(res.body.models.length).toBe(3);
      expect(res.body.models[0].prices.length).toBeGreaterThan(0);
    });
    it('404s an unknown provider', async () => {
      await http.get('/v1/providers/does-not-exist').expect(404);
    });
  });

  // ---------------------------------------------------------------- models
  describe('models', () => {
    it('lists all 32 models', async () => {
      const res = await http.get('/v1/models').expect(200);
      expect(res.body).toHaveLength(32);
    });
    it('filters by provider', async () => {
      const res = await http.get('/v1/models?provider=fal').expect(200);
      expect(res.body.every((m: any) => m.providerId === 'fal')).toBe(true);
      expect(res.body).toHaveLength(8);
    });
    it('filters by mediaType', async () => {
      const res = await http.get('/v1/models?mediaType=video').expect(200);
      expect(res.body.every((m: any) => m.mediaType === 'video')).toBe(true);
    });
    it('filters by freshness (all seeded fresh)', async () => {
      const fresh = await http.get('/v1/models?freshness=fresh').expect(200);
      expect(fresh.body.length).toBeGreaterThan(0);
      const failed = await http.get('/v1/models?freshness=failed').expect(200);
      expect(failed.body).toHaveLength(0);
    });
    it('rejects an invalid enum filter (400)', async () => {
      await http.get('/v1/models?mediaType=hologram').expect(400);
    });
    it('resolves a slashed model id', async () => {
      const res = await http.get('/v1/models/openai/dall-e-3').expect(200);
      expect(res.body.id).toBe('openai/dall-e-3');
      expect(res.body.prices[0].amountUsd).toBe(0.04);
    });
    it('404s an unknown model', async () => {
      await http.get('/v1/models/openai/nope').expect(404);
    });
    it('returns price history (seed baseline) with a limit', async () => {
      const res = await http.get('/v1/models/openai/dall-e-3/history?limit=5').expect(200);
      expect(res.body.length).toBeGreaterThanOrEqual(1);
      expect(res.body[0]).toHaveProperty('recordedAt');
    });
    it('404s history for an unknown model', async () => {
      await http.get('/v1/models/openai/nope/history').expect(404);
    });
  });

  // --------------------------------------------------------------- pricing
  describe('pricing', () => {
    it('returns the full flat table', async () => {
      const res = await http.get('/v1/pricing').expect(200);
      expect(res.body.length).toBeGreaterThanOrEqual(32);
      expect(res.body[0]).toHaveProperty('amountUsd');
      expect(res.body[0]).toHaveProperty('freshness');
    });
    it('filters by provider, mediaType, unit, source, freshness', async () => {
      expect((await http.get('/v1/pricing?provider=openai').expect(200)).body.every((r: any) => r.providerId === 'openai')).toBe(true);
      expect((await http.get('/v1/pricing?mediaType=image').expect(200)).body.every((r: any) => r.mediaType === 'image')).toBe(true);
      expect((await http.get('/v1/pricing?unit=per_video').expect(200)).body.every((r: any) => r.unit === 'per_video')).toBe(true);
      expect((await http.get('/v1/pricing?source=fallback').expect(200)).body.length).toBeGreaterThan(0);
      expect((await http.get('/v1/pricing?freshness=fresh').expect(200)).body.length).toBeGreaterThan(0);
    });
    it('rejects an invalid unit filter (400)', async () => {
      await http.get('/v1/pricing?unit=per_lightyear').expect(400);
    });
  });

  // ------------------------------------------------------------------ auth
  describe('oauth token', () => {
    it('issues a token for valid credentials', async () => {
      const res = await http.post('/oauth/token').send(creds()).expect(200);
      expect(res.body.token_type).toBe('Bearer');
      expect(res.body.scope).toContain('pricing:admin');
    });
    it('grants a requested scope subset', async () => {
      const res = await http.post('/oauth/token').send(creds({ scope: 'pricing:read' })).expect(200);
      expect(res.body.scope).toBe('pricing:read');
    });
    it('rejects an unsupported grant type (400)', async () => {
      await http.post('/oauth/token').send(creds({ grant_type: 'password' })).expect(400);
    });
    it('rejects bad credentials (401)', async () => {
      await http.post('/oauth/token').send(creds({ client_secret: 'wrong' })).expect(401);
    });
    it('rejects an unknown client (401)', async () => {
      await http.post('/oauth/token').send(creds({ client_id: 'ghost' })).expect(401);
    });
    it('rejects a scope the client does not hold (400 invalid_scope)', async () => {
      await http.post('/oauth/token').send(creds({ scope: 'pricing:superuser' })).expect(400);
    });
    it('rejects a malformed token request body (400)', async () => {
      await http.post('/oauth/token').send({ grant_type: 'client_credentials' }).expect(400);
    });
  });

  // ------------------------------------------------------------ protection
  describe('route protection', () => {
    it('401 without a token', async () => {
      await http.patch('/v1/admin/providers/openai').send({ notes: 'x' }).expect(401);
    });
    it('401 with a malformed token', async () => {
      await http
        .patch('/v1/admin/providers/openai')
        .set('Authorization', 'Bearer not-a-jwt')
        .send({ notes: 'x' })
        .expect(401);
    });
    it('403 when the token lacks pricing:admin', async () => {
      // Create a read-only client, get its token, attempt an admin write.
      const created = await http
        .post('/v1/admin/clients')
        .set(auth())
        .send({ name: 'reader', scopes: ['pricing:read'] })
        .expect(201);
      const tokenRes = await http
        .post('/oauth/token')
        .send({
          grant_type: 'client_credentials',
          client_id: created.body.clientId,
          client_secret: created.body.clientSecret,
        })
        .expect(200);
      await http
        .patch('/v1/admin/providers/openai')
        .set('Authorization', `Bearer ${tokenRes.body.access_token}`)
        .send({ notes: 'nope' })
        .expect(403);
    });
  });

  // ----------------------------------------------------------- admin writes
  describe('admin: clients', () => {
    it('creates a client whose credentials work', async () => {
      const created = await http
        .post('/v1/admin/clients')
        .set(auth())
        .send({ name: 'integration', scopes: ['pricing:read', 'pricing:admin'] })
        .expect(201);
      expect(created.body.clientId).toMatch(/^client_/);
      expect(created.body.clientSecret).toBeDefined();
      await http
        .post('/oauth/token')
        .send({
          grant_type: 'client_credentials',
          client_id: created.body.clientId,
          client_secret: created.body.clientSecret,
        })
        .expect(200);
    });
    it('rejects creating a client with empty scopes (400)', async () => {
      await http.post('/v1/admin/clients').set(auth()).send({ name: 'x', scopes: [] }).expect(400);
    });
  });

  describe('admin: price upsert', () => {
    it('creates a manual price component', async () => {
      const res = await http
        .post('/v1/admin/models/openai/dall-e-3/price')
        .set(auth())
        .send({ unit: 'per_image', amountUsd: 0.045 })
        .expect(201);
      expect(res.body.amountUsd).toBe(0.045);
      expect(res.body.source).toBe('manual');

      const model = await http.get('/v1/models/openai/dall-e-3').expect(200);
      const perImage = model.body.prices.find((p: any) => p.unit === 'per_image');
      expect(perImage.amountUsd).toBe(0.045);
      expect(perImage.source).toBe('manual');
    });
    it('updates the existing component on a second upsert (history grows)', async () => {
      await http
        .post('/v1/admin/models/openai/dall-e-3/price')
        .set(auth())
        .send({ unit: 'per_image', amountUsd: 0.05, currency: 'USD', unitAmount: 1 })
        .expect(201);
      const history = await http.get('/v1/models/openai/dall-e-3/history').expect(200);
      expect(history.body.filter((h: any) => h.source === 'manual').length).toBeGreaterThanOrEqual(2);
    });
    it('creates a separate tiered component', async () => {
      const res = await http
        .post('/v1/admin/models/openai/dall-e-3/price')
        .set(auth())
        .send({ unit: 'per_image', amountUsd: 0.08, tier: { quality: 'hd' } })
        .expect(201);
      expect(res.body.tier).toEqual({ quality: 'hd' });
      const model = await http.get('/v1/models/openai/dall-e-3').expect(200);
      expect(model.body.prices.filter((p: any) => p.unit === 'per_image').length).toBe(2);
    });
    it('404s an unknown model', async () => {
      await http
        .post('/v1/admin/models/openai/ghost/price')
        .set(auth())
        .send({ unit: 'per_image', amountUsd: 1 })
        .expect(404);
    });
    it('400s a malformed body', async () => {
      await http
        .post('/v1/admin/models/openai/dall-e-3/price')
        .set(auth())
        .send({ unit: 'per_image' })
        .expect(400);
    });
  });

  describe('admin: provider patch + scrape', () => {
    it('triggers a scrape on a stub (manual) provider -> skipped', async () => {
      const res = await http.post('/v1/admin/providers/luma/scrape').set(auth()).expect(201);
      expect(res.body.status).toBe('skipped');
    });
    it('triggers a scrape on a real scraped provider -> failed (no network) but keeps prices', async () => {
      const res = await http.post('/v1/admin/providers/fal/scrape').set(auth()).expect(201);
      expect(['failed', 'partial']).toContain(res.body.status);
      // Price preserved, freshness degraded to stale.
      const stale = await http.get('/v1/pricing?provider=fal&freshness=stale').expect(200);
      expect(stale.body.length).toBeGreaterThan(0);
      expect(stale.body[0].amountUsd).toBeGreaterThan(0);
    });
    it('404s a scrape for an unknown provider', async () => {
      await http.post('/v1/admin/providers/ghost/scrape').set(auth()).expect(404);
    });
    it('patches provider mode, clears cron, sets url + notes', async () => {
      const res = await http
        .patch('/v1/admin/providers/openai')
        .set(auth())
        .send({ mode: 'manual', cronSchedule: '', pricingUrl: 'https://example.com/p', notes: 'patched' })
        .expect(200);
      expect(res.body.mode).toBe('manual');
      expect(res.body.cronSchedule).toBeNull();
      expect(res.body.pricingUrl).toBe('https://example.com/p');
    });
    it('404s a patch for an unknown provider', async () => {
      await http.patch('/v1/admin/providers/ghost').set(auth()).send({ notes: 'x' }).expect(404);
    });
  });

  // A scrape can't reach the live network in tests, so we inject a fake fetch
  // into ScrapeService that returns a fixture pricing page. This exercises the
  // success path end-to-end: parse → diff → persist → freshness recovery.
  describe('admin: scrape success path (injected fetch)', () => {
    const FAL_FIXTURE = `
      <html><body><table>
        <tr><th>Model</th><th>Price</th></tr>
        <tr><td>FLUX.1 [pro]</td><td>$0.055 / image</td></tr>
        <tr><td>FLUX.1 [dev]</td><td>$0.030 / image</td></tr>
        <tr><td>FLUX.1 [schnell]</td><td>$0.004 / image</td></tr>
        <tr><td>SDXL</td><td>$0.018 / image</td></tr>
        <tr><td>SD 3.5 Medium</td><td>$0.027 / image</td></tr>
        <tr><td>Recraft V3</td><td>$0.042 / image</td></tr>
        <tr><td>AuraFlow</td><td>$0.021 / image</td></tr>
      </table></body></html>`;

    it('updates prices and recovers freshness on a successful scrape', async () => {
      const svc = app.get(ScrapeService);
      svc.fetchImpl = (async () =>
        new Response(FAL_FIXTURE, {
          status: 200,
          headers: { 'content-type': 'text/html' },
        })) as unknown as typeof fetch;

      const res = await http.post('/v1/admin/providers/fal/scrape').set(auth()).expect(201);
      expect(res.body.status).toBe('success');
      expect(res.body.componentsUpdated).toBeGreaterThan(0);

      // fal/flux-dev was 0.025 in the seed; the fixture moves it to 0.030.
      const model = await http.get('/v1/models/fal/flux-dev').expect(200);
      const price = model.body.prices.find((p: any) => p.unit === 'per_image');
      expect(price.amountUsd).toBe(0.03);
      expect(price.source).toBe('scraped');
      expect(price.freshness).toBe('fresh');

      // The change is recorded in history.
      const history = await http.get('/v1/models/fal/flux-dev/history').expect(200);
      expect(history.body.some((h: any) => h.source === 'scraped')).toBe(true);
    });
  });
});
