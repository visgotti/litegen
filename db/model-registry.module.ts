import { Module } from '@nestjs/common';
import { TypeOrmModule } from '@nestjs/typeorm';
import { ModelRegistryService } from './model-registry.service';
import { AiModel } from '../../db/modules/ai-model/ai-model.entity';
import { AiProvider } from '../../db/modules/ai-provider/ai-provider.entity';

@Module({
  imports: [TypeOrmModule.forFeature([AiModel, AiProvider])],
  providers: [ModelRegistryService],
  exports: [ModelRegistryService],
})
export class ModelRegistryModule {}
