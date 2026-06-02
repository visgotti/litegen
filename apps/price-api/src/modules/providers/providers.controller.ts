import { Controller, Get, Param } from '@nestjs/common';
import { ApiOkResponse, ApiOperation, ApiParam, ApiTags } from '@nestjs/swagger';
import { Public } from '../../common/decorators/public.decorator';
import { ProviderDetailDto, ProviderDto } from './dto/provider.dto';
import { ProvidersService } from './providers.service';

@ApiTags('providers')
@Controller({ path: 'providers', version: '1' })
export class ProvidersController {
  constructor(private readonly providersService: ProvidersService) {}

  @Public()
  @Get()
  @ApiOperation({ summary: 'List all providers with their refresh mode and freshness metadata.' })
  @ApiOkResponse({ type: [ProviderDto] })
  list(): Promise<ProviderDto[]> {
    return this.providersService.list();
  }

  @Public()
  @Get(':id')
  @ApiOperation({ summary: 'Get one provider with all its models and current prices.' })
  @ApiParam({ name: 'id', example: 'openai' })
  @ApiOkResponse({ type: ProviderDetailDto })
  get(@Param('id') id: string): Promise<ProviderDetailDto> {
    return this.providersService.get(id);
  }
}
