import { Controller, DefaultValuePipe, Get, Param, ParseIntPipe, Query } from '@nestjs/common';
import { ApiOkResponse, ApiOperation, ApiParam, ApiQuery, ApiTags } from '@nestjs/swagger';
import { ModelDto } from '../../common/dto/model.dto';
import { Public } from '../../common/decorators/public.decorator';
import { ListModelsQuery } from './dto/list-models.query';
import { PriceHistoryDto } from './dto/price-history.dto';
import { ModelsService } from './models.service';

@ApiTags('models')
@Controller({ path: 'models', version: '1' })
export class ModelsController {
  constructor(private readonly modelsService: ModelsService) {}

  @Public()
  @Get()
  @ApiOperation({ summary: 'List models with their current price components.' })
  @ApiOkResponse({ type: [ModelDto] })
  list(@Query() query: ListModelsQuery): Promise<ModelDto[]> {
    return this.modelsService.list(query);
  }

  // Model ids contain a slash (`provider/name`), so they are addressed as two
  // path segments and reconstructed here.
  @Public()
  @Get(':provider/:name')
  @ApiOperation({ summary: 'Get a single model by its fully-qualified id (provider/name).' })
  @ApiParam({ name: 'provider', example: 'openai' })
  @ApiParam({ name: 'name', example: 'dall-e-3' })
  @ApiOkResponse({ type: ModelDto })
  get(@Param('provider') provider: string, @Param('name') name: string): Promise<ModelDto> {
    return this.modelsService.get(`${provider}/${name}`);
  }

  @Public()
  @Get(':provider/:name/history')
  @ApiOperation({ summary: 'Price-change history for a model (most recent first).' })
  @ApiParam({ name: 'provider', example: 'openai' })
  @ApiParam({ name: 'name', example: 'dall-e-3' })
  @ApiQuery({ name: 'limit', required: false, example: 50 })
  @ApiOkResponse({ type: [PriceHistoryDto] })
  history(
    @Param('provider') provider: string,
    @Param('name') name: string,
    @Query('limit', new DefaultValuePipe(50), ParseIntPipe) limit: number,
  ): Promise<PriceHistoryDto[]> {
    return this.modelsService.history(`${provider}/${name}`, Math.min(Math.max(limit, 1), 500));
  }
}
