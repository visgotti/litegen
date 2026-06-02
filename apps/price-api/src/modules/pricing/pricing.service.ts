import { Injectable } from '@nestjs/common';
import { InjectRepository } from '@nestjs/typeorm';
import { Repository } from 'typeorm';
import { ModelPriceEntity } from '../../entities';
import { PricingQuery } from './dto/pricing-query.dto';
import { PricingRowDto } from './dto/pricing-row.dto';

@Injectable()
export class PricingService {
  constructor(
    @InjectRepository(ModelPriceEntity)
    private readonly priceRepo: Repository<ModelPriceEntity>,
  ) {}

  async query(query: PricingQuery): Promise<PricingRowDto[]> {
    const qb = this.priceRepo
      .createQueryBuilder('price')
      .innerJoinAndSelect('price.model', 'model')
      .orderBy('model.id', 'ASC')
      .addOrderBy('price.unit', 'ASC');

    if (query.provider) {
      qb.andWhere('model.providerId = :provider', { provider: query.provider });
    }
    if (query.mediaType) {
      qb.andWhere('model.mediaType = :mediaType', { mediaType: query.mediaType });
    }
    if (query.unit) {
      qb.andWhere('price.unit = :unit', { unit: query.unit });
    }
    if (query.freshness) {
      qb.andWhere('price.freshness = :freshness', { freshness: query.freshness });
    }
    if (query.source) {
      qb.andWhere('price.source = :source', { source: query.source });
    }

    const rows = await qb.getMany();
    return rows.map((r) => PricingRowDto.fromEntity(r));
  }
}
