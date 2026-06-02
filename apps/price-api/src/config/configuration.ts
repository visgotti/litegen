/**
 * Typed application configuration, derived from validated environment variables.
 * Consumed via `ConfigService<AppConfig, true>` for end-to-end type safety.
 */
export interface AppConfig {
  nodeEnv: 'development' | 'test' | 'production';
  port: number;
  corsOrigins: string[] | '*';
  database: {
    url: string;
    synchronize: boolean;
    logging: boolean;
  };
  jwt: {
    secret: string;
    ttlSeconds: number;
    issuer: string;
    audience: string;
  };
  bootstrap: {
    clientId: string;
    clientSecret: string | null;
    scopes: string[];
  };
  throttle: {
    ttlSeconds: number;
    limit: number;
    authLimit: number;
  };
  scraping: {
    enabled: boolean;
    timeoutMs: number;
    staleAfterFailures: number;
  };
  litegenModelsDir: string;
}

function splitCsv(value: string | undefined): string[] {
  return (value ?? '')
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean);
}

export default (): AppConfig => {
  const corsRaw = (process.env.CORS_ORIGINS ?? '*').trim();
  return {
    nodeEnv: (process.env.NODE_ENV ?? 'development') as AppConfig['nodeEnv'],
    port: parseInt(process.env.PORT ?? '4100', 10),
    corsOrigins: corsRaw === '*' ? '*' : splitCsv(corsRaw),
    database: {
      url: process.env.DATABASE_URL as string,
      synchronize: process.env.DB_SYNCHRONIZE === 'true',
      logging: process.env.DB_LOGGING === 'true',
    },
    jwt: {
      secret: process.env.JWT_SECRET as string,
      ttlSeconds: parseInt(process.env.JWT_TTL_SECONDS ?? '3600', 10),
      issuer: process.env.JWT_ISSUER ?? 'litegen-price-api',
      audience: process.env.JWT_AUDIENCE ?? 'litegen-price-api',
    },
    bootstrap: {
      clientId: process.env.BOOTSTRAP_CLIENT_ID ?? 'bootstrap-admin',
      clientSecret: process.env.BOOTSTRAP_CLIENT_SECRET || null,
      scopes: splitCsv(process.env.BOOTSTRAP_CLIENT_SCOPES ?? 'pricing:read,pricing:admin'),
    },
    throttle: {
      ttlSeconds: parseInt(process.env.THROTTLE_TTL_SECONDS ?? '60', 10),
      limit: parseInt(process.env.THROTTLE_LIMIT ?? '120', 10),
      authLimit: parseInt(process.env.THROTTLE_AUTH_LIMIT ?? '10', 10),
    },
    scraping: {
      enabled: process.env.SCRAPING_ENABLED !== 'false',
      timeoutMs: parseInt(process.env.SCRAPE_TIMEOUT_MS ?? '15000', 10),
      staleAfterFailures: parseInt(process.env.SCRAPE_STALE_AFTER_FAILURES ?? '3', 10),
    },
    litegenModelsDir: process.env.LITEGEN_MODELS_DIR ?? '../../models',
  };
};
