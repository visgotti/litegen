import { Module } from '@nestjs/common';
import { TypeOrmModule } from '@nestjs/typeorm';
import { ModelPriceEntity } from '../../entities';
import { PricingController } from './pricing.controller';
import { PricingService } from './pricing.service';

@Module({
  imports: [TypeOrmModule.forFeature([ModelPriceEntity])],
  controllers: [PricingController],
  providers: [PricingService],
})
export class PricingModule {}
