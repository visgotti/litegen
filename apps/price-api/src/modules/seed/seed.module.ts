import { Module } from '@nestjs/common';
import { TypeOrmModule } from '@nestjs/typeorm';
import {
  ModelEntity,
  ModelPriceEntity,
  PriceHistoryEntity,
  ProviderEntity,
} from '../../entities';
import { AuthModule } from '../auth/auth.module';
import { SeedService } from './seed.service';

@Module({
  imports: [
    TypeOrmModule.forFeature([
      ProviderEntity,
      ModelEntity,
      ModelPriceEntity,
      PriceHistoryEntity,
    ]),
    AuthModule,
  ],
  providers: [SeedService],
  exports: [SeedService],
})
export class SeedModule {}
