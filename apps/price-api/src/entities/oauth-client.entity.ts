import {
  Column,
  CreateDateColumn,
  Entity,
  Index,
  PrimaryGeneratedColumn,
  UpdateDateColumn,
} from 'typeorm';
import { Scope } from '../common/enums';

/**
 * An OAuth2 client (machine-to-machine). Authenticates via the
 * client-credentials grant to obtain a short-lived JWT. The secret is stored
 * only as an argon2 hash — the plaintext is shown once at creation time.
 */
@Entity('oauth_clients')
export class OAuthClientEntity {
  @PrimaryGeneratedColumn('uuid')
  id!: string;

  @Index({ unique: true })
  @Column({ type: 'varchar', length: 128 })
  clientId!: string;

  @Column({ type: 'varchar', length: 256 })
  clientSecretHash!: string;

  @Column({ type: 'varchar', length: 128 })
  name!: string;

  /** Scopes this client may request, stored as a comma-separated list. */
  @Column({ type: 'simple-array' })
  scopes!: Scope[];

  @Column({ type: 'boolean', default: true })
  active!: boolean;

  @Column({ type: 'timestamptz', nullable: true })
  lastUsedAt!: Date | null;

  @CreateDateColumn({ type: 'timestamptz' })
  createdAt!: Date;

  @UpdateDateColumn({ type: 'timestamptz' })
  updatedAt!: Date;
}
