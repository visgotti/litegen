import { Module } from '@nestjs/common';
import { TypeOrmModule } from '@nestjs/typeorm';
import { ModelEntity, ProviderEntity } from '../../entities';
import { ProvidersController } from './providers.controller';
import { ProvidersService } from './providers.service';

@Module({
  imports: [TypeOrmModule.forFeature([ProviderEntity, ModelEntity])],
  controllers: [ProvidersController],
  providers: [ProvidersService],
})
export class ProvidersModule {}
