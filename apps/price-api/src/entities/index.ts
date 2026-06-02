import { ModelEntity } from './model.entity';
import { ModelPriceEntity } from './model-price.entity';
import { OAuthClientEntity } from './oauth-client.entity';
import { PriceHistoryEntity } from './price-history.entity';
import { ProviderEntity } from './provider.entity';
import { ScrapeRunEntity } from './scrape-run.entity';

export {
  ModelEntity,
  ModelPriceEntity,
  OAuthClientEntity,
  PriceHistoryEntity,
  ProviderEntity,
  ScrapeRunEntity,
};

/** All persistent entities, for TypeORM registration. */
export const ENTITIES = [
  ProviderEntity,
  ModelEntity,
  ModelPriceEntity,
  PriceHistoryEntity,
  ScrapeRunEntity,
  OAuthClientEntity,
];
