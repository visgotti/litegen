import {
  Column,
  CreateDateColumn,
  Entity,
  Index,
  JoinColumn,
  ManyToOne,
  OneToMany,
  PrimaryColumn,
  UpdateDateColumn,
} from 'typeorm';
import { MediaType, ProviderMode } from '../common/enums';
import { ModelPriceEntity } from './model-price.entity';
import { ProviderEntity } from './provider.entity';

/**
 * A single model offered by a provider (e.g. `openai/dall-e-3`). Carries the
 * effective refresh mode (provider default unless `modeOverride` is set) and
 * owns one or more {@link ModelPriceEntity} components.
 */
@Entity('models')
export class ModelEntity {
  /** Fully-qualified id, matches litegen model id (e.g. `openai/dall-e-3`). */
  @PrimaryColumn({ type: 'varchar', length: 128 })
  id!: string;

  @Index()
  @Column({ type: 'varchar', length: 64 })
  providerId!: string;

  @ManyToOne(() => ProviderEntity, (provider) => provider.models, {
    onDelete: 'CASCADE',
  })
  @JoinColumn({ name: 'providerId' })
  provider!: ProviderEntity;

  @Column({ type: 'varchar', length: 128 })
  displayName!: string;

  @Column({ type: 'varchar', length: 16 })
  mediaType!: MediaType;

  /** Overrides the provider default mode for just this model; null = inherit. */
  @Column({ type: 'varchar', length: 16, nullable: true })
  modeOverride!: ProviderMode | null;

  @Column({ type: 'boolean', default: true })
  active!: boolean;

  @OneToMany(() => ModelPriceEntity, (price) => price.model)
  prices!: ModelPriceEntity[];

  @CreateDateColumn({ type: 'timestamptz' })
  createdAt!: Date;

  @UpdateDateColumn({ type: 'timestamptz' })
  updatedAt!: Date;
}
