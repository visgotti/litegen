import { Injectable, Logger, OnModuleInit } from '@nestjs/common';
import { ConfigService } from '@nestjs/config';
import { SchedulerRegistry } from '@nestjs/schedule';
import { CronJob } from 'cron';
import { AppConfig } from '../../config/configuration';
import { ProviderMode } from '../../common/enums';
import { PROVIDER_REGISTRY } from '../../providers/registry';
import { ScrapeService } from './scrape.service';

/**
 * Registers a cron job per scraped provider from its `cronSchedule`, when
 * scraping is enabled. Jobs are fire-and-forget; failures are caught and logged
 * (the scrape service already records them as failed runs and degrades
 * freshness), so one provider's outage never crashes the scheduler.
 */
@Injectable()
export class SchedulerService implements OnModuleInit {
  private readonly logger = new Logger(SchedulerService.name);

  constructor(
    private readonly configService: ConfigService<AppConfig, true>,
    private readonly schedulerRegistry: SchedulerRegistry,
    private readonly scrapeService: ScrapeService,
  ) {}

  onModuleInit(): void {
    const { enabled } = this.configService.get('scraping', { infer: true });
    if (!enabled) {
      this.logger.log('Scraping disabled (SCRAPING_ENABLED=false); no cron jobs registered.');
      return;
    }

    for (const def of PROVIDER_REGISTRY) {
      const { id, mode, cronSchedule } = def.config;
      if (mode !== ProviderMode.SCRAPED || !cronSchedule || !def.scraper.implemented) {
        continue;
      }
      const name = `scrape:${id}`;
      const job = new CronJob(cronSchedule, () => {
        this.scrapeService
          .scrapeProvider(id)
          .then((run) =>
            this.logger.log(`Scheduled scrape ${id} -> ${run.status} (${run.componentsUpdated} updated)`),
          )
          .catch((err) => this.logger.error(`Scheduled scrape ${id} crashed: ${err}`));
      });
      this.schedulerRegistry.addCronJob(name, job);
      job.start();
      this.logger.log(`Registered cron "${cronSchedule}" for provider ${id}`);
    }
  }
}
