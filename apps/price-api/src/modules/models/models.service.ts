import { Injectable, NotFoundException } from '@nestjs/common';
import { InjectRepository } from '@nestjs/typeorm';
import { FindOptionsWhere, Repository } from 'typeorm';
import { ModelDto } from '../../common/dto/model.dto';
import { ModelEntity, PriceHistoryEntity } from '../../entities';
import { ListModelsQuery } from './dto/list-models.query';
import { PriceHistoryDto } from './dto/price-history.dto';

@Injectable()
export class ModelsService {
  constructor(
    @InjectRepository(ModelEntity)
    private readonly modelRepo: Repository<ModelEntity>,
    @InjectRepository(PriceHistoryEntity)
    private readonly historyRepo: Repository<PriceHistoryEntity>,
  ) {}

  async list(query: ListModelsQuery): Promise<ModelDto[]> {
    const where: FindOptionsWhere<ModelEntity> = {};
    if (query.provider) {
      where.providerId = query.provider;
    }
    if (query.mediaType) {
      where.mediaType = query.mediaType;
    }

    const models = await this.modelRepo.find({
      where,
      relations: { provider: true, prices: true },
      order: { id: 'ASC' },
    });

    const filtered = query.freshness
      ? models.filter((m) => (m.prices ?? []).some((p) => p.freshness === query.freshness))
      : models;

    return filtered.map((m) => ModelDto.fromEntity(m, m.provider));
  }

  async get(id: string): Promise<ModelDto> {
    const model = await this.modelRepo.findOne({
      where: { id },
      relations: { provider: true, prices: true },
    });
    if (!model) {
      throw new NotFoundException(`Model "${id}" not found`);
    }
    return ModelDto.fromEntity(model, model.provider);
  }

  async history(id: string, limit: number): Promise<PriceHistoryDto[]> {
    const exists = await this.modelRepo.exists({ where: { id } });
    if (!exists) {
      throw new NotFoundException(`Model "${id}" not found`);
    }
    const rows = await this.historyRepo.find({
      where: { modelId: id },
      order: { recordedAt: 'DESC' },
      take: limit,
    });
    return rows.map((r) => PriceHistoryDto.fromEntity(r));
  }
}
