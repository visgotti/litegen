import { Column, Entity, Index, PrimaryGeneratedColumn } from 'typeorm';
import { PriceSource, PriceUnit } from '../common/enums';
import { NumericTransformer } from '../common/numeric.transformer';

/**
 * Append-only audit log: one row per observed price change for a model
 * component. Enables "what was the price on date X" queries and provides a
 * tamper-evident trail of scrape and manual updates.
 */
@Entity('price_history')
@Index('idx_price_history_model_recorded', ['modelId', 'recordedAt'])
export class PriceHistoryEntity {
  @PrimaryGeneratedColumn('uuid')
  id!: string;

  @Column({ type: 'varchar', length: 128 })
  modelId!: string;

  @Column({ type: 'varchar', length: 24 })
  unit!: PriceUnit;

  @Column({ type: 'numeric', precision: 12, scale: 4, default: 1, transformer: new NumericTransformer() })
  unitAmount!: number;

  @Column({ type: 'numeric', precision: 14, scale: 6, transformer: new NumericTransformer() })
  amountUsd!: number;

  @Column({ type: 'varchar', length: 8, default: 'USD' })
  currency!: string;

  @Column({ type: 'jsonb', nullable: true })
  tier!: Record<string, string> | null;

  @Column({ type: 'varchar', length: 256, default: '*' })
  tierKey!: string;

  @Column({ type: 'varchar', length: 16 })
  source!: PriceSource;

  /** The scrape run that produced this change, if any. */
  @Column({ type: 'uuid', nullable: true })
  scrapeRunId!: string | null;

  /** Human-readable context, e.g. "0.040 -> 0.045" or "manual upsert by client X". */
  @Column({ type: 'varchar', length: 256, nullable: true })
  note!: string | null;

  @Index()
  @Column({ type: 'timestamptz' })
  recordedAt!: Date;
}
