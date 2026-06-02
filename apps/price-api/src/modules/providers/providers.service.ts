import { Injectable, NotFoundException } from '@nestjs/common';
import { InjectRepository } from '@nestjs/typeorm';
import { Repository } from 'typeorm';
import { ModelEntity, ProviderEntity } from '../../entities';
import { ProviderDetailDto, ProviderDto } from './dto/provider.dto';

@Injectable()
export class ProvidersService {
  constructor(
    @InjectRepository(ProviderEntity)
    private readonly providerRepo: Repository<ProviderEntity>,
    @InjectRepository(ModelEntity)
    private readonly modelRepo: Repository<ModelEntity>,
  ) {}

  async list(): Promise<ProviderDto[]> {
    const providers = await this.providerRepo.find({ order: { id: 'ASC' } });
    const counts = await this.modelRepo
      .createQueryBuilder('m')
      .select('m.providerId', 'providerId')
      .addSelect('COUNT(*)', 'count')
      .groupBy('m.providerId')
      .getRawMany<{ providerId: string; count: string }>();
    const byProvider = new Map(counts.map((c) => [c.providerId, parseInt(c.count, 10)]));
    return providers.map((p) => ProviderDto.fromEntity(p, byProvider.get(p.id) ?? 0));
  }

  async get(id: string): Promise<ProviderDetailDto> {
    const provider = await this.providerRepo.findOne({
      where: { id },
      relations: { models: { prices: true } },
    });
    if (!provider) {
      throw new NotFoundException(`Provider "${id}" not found`);
    }
    return ProviderDetailDto.fromEntityWithModels(provider);
  }
}
