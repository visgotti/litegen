import * as Joi from 'joi';

/**
 * Joi schema applied by `ConfigModule` at boot. A misconfigured environment
 * fails fast and loud instead of surfacing as a confusing runtime error later.
 */
export const envValidationSchema = Joi.object({
  NODE_ENV: Joi.string().valid('development', 'test', 'production').default('development'),
  PORT: Joi.number().port().default(4100),
  CORS_ORIGINS: Joi.string().default('*'),

  DATABASE_URL: Joi.string().uri({ scheme: ['postgres', 'postgresql'] }).required(),
  DB_SYNCHRONIZE: Joi.boolean().truthy('true').falsy('false').default(false),
  DB_LOGGING: Joi.boolean().truthy('true').falsy('false').default(false),

  JWT_SECRET: Joi.string().min(16).required(),
  JWT_TTL_SECONDS: Joi.number().integer().min(60).max(86400).default(3600),
  JWT_ISSUER: Joi.string().default('litegen-price-api'),
  JWT_AUDIENCE: Joi.string().default('litegen-price-api'),

  BOOTSTRAP_CLIENT_ID: Joi.string().default('bootstrap-admin'),
  BOOTSTRAP_CLIENT_SECRET: Joi.string().allow('').default(''),
  BOOTSTRAP_CLIENT_SCOPES: Joi.string().default('pricing:read,pricing:admin'),

  THROTTLE_TTL_SECONDS: Joi.number().integer().min(1).default(60),
  THROTTLE_LIMIT: Joi.number().integer().min(1).default(120),
  THROTTLE_AUTH_LIMIT: Joi.number().integer().min(1).default(10),

  SCRAPING_ENABLED: Joi.boolean().truthy('true').falsy('false').default(true),
  SCRAPE_TIMEOUT_MS: Joi.number().integer().min(1000).default(15000),
  SCRAPE_STALE_AFTER_FAILURES: Joi.number().integer().min(1).default(3),

  LITEGEN_MODELS_DIR: Joi.string().default('../../models'),
});
