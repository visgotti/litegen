import { Controller, Get, Query } from '@nestjs/common';
import { ApiOkResponse, ApiOperation, ApiTags } from '@nestjs/swagger';
import { Public } from '../../common/decorators/public.decorator';
import { PricingQuery } from './dto/pricing-query.dto';
import { PricingRowDto } from './dto/pricing-row.dto';
import { PricingService } from './pricing.service';

@ApiTags('pricing')
@Controller({ path: 'pricing', version: '1' })
export class PricingController {
  constructor(private readonly pricingService: PricingService) {}

  @Public()
  @Get()
  @ApiOperation({
    summary: 'Flat, filterable pricing table across all providers and models.',
    description:
      'The primary endpoint for consumers. Returns one row per model price component. ' +
      'Filter by provider, media type, unit, freshness, or source.',
  })
  @ApiOkResponse({ type: [PricingRowDto] })
  query(@Query() query: PricingQuery): Promise<PricingRowDto[]> {
    return this.pricingService.query(query);
  }
}
