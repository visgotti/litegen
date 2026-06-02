import { Module } from '@nestjs/common';
import { TypeOrmModule } from '@nestjs/typeorm';
import {
  ModelEntity,
  ModelPriceEntity,
  PriceHistoryEntity,
  ProviderEntity,
} from '../../entities';
import { AuthModule } from '../auth/auth.module';
import { ScrapingModule } from '../scraping/scraping.module';
import { AdminController } from './admin.controller';
import { AdminService } from './admin.service';

@Module({
  imports: [
    TypeOrmModule.forFeature([
      ProviderEntity,
      ModelEntity,
      ModelPriceEntity,
      PriceHistoryEntity,
    ]),
    ScrapingModule,
    AuthModule,
  ],
  controllers: [AdminController],
  providers: [AdminService],
})
export class AdminModule {}
