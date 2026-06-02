/**
 * Standalone seed runner. Boots a headless Nest context (which connects to the
 * DB and runs the idempotent SeedService) and prints what was inserted.
 *
 *   npm run seed
 */
import { loadEnv } from '../src/config/load-env';

loadEnv();

import { NestFactory } from '@nestjs/core';
import { AppModule } from '../src/app.module';
import { SeedService } from '../src/modules/seed/seed.service';

async function main(): Promise<void> {
  const app = await NestFactory.createApplicationContext(AppModule, {
    logger: ['error', 'warn', 'log'],
  });
  const seed = app.get(SeedService);
  const result = await seed.seed();
  console.log('Seed result:', JSON.stringify(result));
  await app.close();
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
