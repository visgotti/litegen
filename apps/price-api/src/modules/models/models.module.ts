import { Module } from '@nestjs/common';
import { TypeOrmModule } from '@nestjs/typeorm';
import { ModelEntity, PriceHistoryEntity } from '../../entities';
import { ModelsController } from './models.controller';
import { ModelsService } from './models.service';

@Module({
  imports: [TypeOrmModule.forFeature([ModelEntity, PriceHistoryEntity])],
  controllers: [ModelsController],
  providers: [ModelsService],
})
export class ModelsModule {}
