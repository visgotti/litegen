import {
  Column,
  CreateDateColumn,
  Entity,
  OneToMany,
  PrimaryColumn,
  UpdateDateColumn,
} from 'typeorm';
import { ProviderMode, ScrapeStatus } from '../common/enums';
import { ModelEntity } from './model.entity';

/**
 * A pricing provider (e.g. `openai`, `replicate`). The `mode` is the default
 * refresh strategy for all the provider's models; individual models may
 * override it via {@link ModelEntity.modeOverride}.
 */
@Entity('providers')
export class ProviderEntity {
  /** Stable slug, matches the litegen provider id (e.g. `openai`). */
  @PrimaryColumn({ type: 'varchar', length: 64 })
  id!: string;

  @Column({ type: 'varchar', length: 128 })
  displayName!: string;

  @Column({ type: 'varchar', length: 16, default: ProviderMode.MANUAL })
  mode!: ProviderMode;

  /** Cron expression (node-cron syntax) for scheduled scrapes; null = none. */
  @Column({ type: 'varchar', length: 64, nullable: true })
  cronSchedule!: string | null;

  /** Public pricing page / API used by the scraper, for provenance. */
  @Column({ type: 'varchar', length: 512, nullable: true })
  pricingUrl!: string | null;

  @Column({ type: 'text', nullable: true })
  notes!: string | null;

  /** Whether a real scraper is wired up (vs. a not-yet-implemented stub). */
  @Column({ type: 'boolean', default: false })
  scraperImplemented!: boolean;

  @Column({ type: 'timestamptz', nullable: true })
  lastScrapedAt!: Date | null;

  @Column({ type: 'varchar', length: 16, nullable: true })
  lastScrapeStatus!: ScrapeStatus | null;

  @Column({ type: 'int', default: 0 })
  consecutiveFailures!: number;

  @OneToMany(() => ModelEntity, (model) => model.provider)
  models!: ModelEntity[];

  @CreateDateColumn({ type: 'timestamptz' })
  createdAt!: Date;

  @UpdateDateColumn({ type: 'timestamptz' })
  updatedAt!: Date;
}
