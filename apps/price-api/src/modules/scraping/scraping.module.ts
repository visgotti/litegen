import { Module } from '@nestjs/common';
import { TypeOrmModule } from '@nestjs/typeorm';
import {
  ModelEntity,
  ModelPriceEntity,
  PriceHistoryEntity,
  ProviderEntity,
  ScrapeRunEntity,
} from '../../entities';
import { ScrapeService } from './scrape.service';
import { SchedulerService } from './scheduler.service';

/**
 * Owns price refresh: the {@link ScrapeService} (resilient persistence) and the
 * {@link SchedulerService} (cron registration). `ScrapeService` is exported so
 * the admin module can trigger on-demand scrapes.
 */
@Module({
  imports: [
    TypeOrmModule.forFeature([
      ProviderEntity,
      ModelEntity,
      ModelPriceEntity,
      PriceHistoryEntity,
      ScrapeRunEntity,
    ]),
  ],
  providers: [ScrapeService, SchedulerService],
  exports: [ScrapeService],
})
export class ScrapingModule {}
