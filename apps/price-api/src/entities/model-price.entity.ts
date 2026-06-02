import {
  Column,
  CreateDateColumn,
  Entity,
  Index,
  JoinColumn,
  ManyToOne,
  PrimaryGeneratedColumn,
  UpdateDateColumn,
} from 'typeorm';
import { Freshness, PriceSource, PriceUnit } from '../common/enums';
import { NumericTransformer } from '../common/numeric.transformer';
import { ModelEntity } from './model.entity';

/**
 * The current price of one billing component of a model. A model may have
 * several (e.g. a video model priced per-second at different resolution tiers).
 * Uniqueness is (modelId, unit, tierKey).
 */
@Entity('model_prices')
@Index('uq_model_price_component', ['modelId', 'unit', 'tierKey'], { unique: true })
export class ModelPriceEntity {
  @PrimaryGeneratedColumn('uuid')
  id!: string;

  @Index()
  @Column({ type: 'varchar', length: 128 })
  modelId!: string;

  @ManyToOne(() => ModelEntity, (model) => model.prices, { onDelete: 'CASCADE' })
  @JoinColumn({ name: 'modelId' })
  model!: ModelEntity;

  @Column({ type: 'varchar', length: 24 })
  unit!: PriceUnit;

  /** The denominator the price applies to (e.g. per 1 image, per 1 second). */
  @Column({ type: 'numeric', precision: 12, scale: 4, default: 1, transformer: new NumericTransformer() })
  unitAmount!: number;

  @Column({ type: 'numeric', precision: 14, scale: 6, transformer: new NumericTransformer() })
  amountUsd!: number;

  @Column({ type: 'varchar', length: 8, default: 'USD' })
  currency!: string;

  /** Optional qualifiers e.g. `{resolution:"1080p", quality:"hd"}`. */
  @Column({ type: 'jsonb', nullable: true })
  tier!: Record<string, string> | null;

  /** Canonical key derived from {@link tier}; `*` when no tier. */
  @Column({ type: 'varchar', length: 256, default: '*' })
  tierKey!: string;

  @Column({ type: 'varchar', length: 16, default: PriceSource.FALLBACK })
  source!: PriceSource;

  @Column({ type: 'varchar', length: 16, default: Freshness.FRESH })
  freshness!: Freshness;

  /** Consecutive scrape failures affecting this component. */
  @Column({ type: 'int', default: 0 })
  consecutiveFailures!: number;

  /** When the served amount was last confirmed by a successful source. */
  @Column({ type: 'timestamptz' })
  lastUpdatedAt!: Date;

  /** When a refresh was last attempted (success or failure). */
  @Column({ type: 'timestamptz', nullable: true })
  lastAttemptAt!: Date | null;

  @CreateDateColumn({ type: 'timestamptz' })
  createdAt!: Date;

  @UpdateDateColumn({ type: 'timestamptz' })
  updatedAt!: Date;
}
