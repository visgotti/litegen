import { DataSource } from 'typeorm';
import { loadEnv } from '../config/load-env';
import { ENTITIES } from '../entities';

loadEnv();

/**
 * Standalone TypeORM DataSource used by the migration CLI (`npm run typeorm`).
 * The running application configures TypeORM through `TypeOrmModule.forRootAsync`
 * in `AppModule`; both share the same entity set and connection string.
 */
export const AppDataSource = new DataSource({
  type: 'postgres',
  url: process.env.DATABASE_URL,
  entities: ENTITIES,
  migrations: [__dirname + '/migrations/*.{ts,js}'],
  synchronize: false,
  logging: process.env.DB_LOGGING === 'true',
});
