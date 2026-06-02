import { Column, Entity, Index, PrimaryGeneratedColumn } from 'typeorm';
import { ScrapeStatus } from '../common/enums';

/**
 * Record of a single scrape attempt for one provider. Drives the freshness
 * model and gives operators an audit trail of what ran, when, and why it failed.
 */
@Entity('scrape_runs')
@Index('idx_scrape_run_provider_started', ['providerId', 'startedAt'])
export class ScrapeRunEntity {
  @PrimaryGeneratedColumn('uuid')
  id!: string;

  @Column({ type: 'varchar', length: 64 })
  providerId!: string;

  @Column({ type: 'varchar', length: 16 })
  status!: ScrapeStatus;

  @Column({ type: 'timestamptz' })
  startedAt!: Date;

  @Column({ type: 'timestamptz', nullable: true })
  finishedAt!: Date | null;

  @Column({ type: 'int', nullable: true })
  durationMs!: number | null;

  /** Number of price components written/updated by this run. */
  @Column({ type: 'int', default: 0 })
  componentsUpdated!: number;

  /** Number of price components the scraper returned. */
  @Column({ type: 'int', default: 0 })
  componentsSeen!: number;

  @Column({ type: 'varchar', length: 512, nullable: true })
  sourceUrl!: string | null;

  @Column({ type: 'text', nullable: true })
  error!: string | null;
}
