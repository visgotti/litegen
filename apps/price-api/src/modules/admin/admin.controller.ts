import { Body, Controller, Param, Patch, Post } from '@nestjs/common';
import {
  ApiBearerAuth,
  ApiCreatedResponse,
  ApiForbiddenResponse,
  ApiOkResponse,
  ApiOperation,
  ApiParam,
  ApiTags,
  ApiUnauthorizedResponse,
} from '@nestjs/swagger';
import { PriceComponentDto } from '../../common/dto/price-component.dto';
import { CurrentClient } from '../../common/decorators/current-client.decorator';
import { Scopes } from '../../common/decorators/scopes.decorator';
import { AuthPrincipal } from '../../common/auth-principal';
import { Scope } from '../../common/enums';
import { AuthService } from '../auth/auth.service';
import { ProviderDto } from '../providers/dto/provider.dto';
import { AdminService } from './admin.service';
import { CreateClientDto, CreatedClientDto } from './dto/create-client.dto';
import { ScrapeRunDto } from './dto/scrape-run.dto';
import { UpdateProviderDto } from './dto/update-provider.dto';
import { UpsertPriceDto } from './dto/upsert-price.dto';

@ApiTags('admin')
@ApiBearerAuth()
@ApiUnauthorizedResponse({ description: 'Missing or invalid bearer token.' })
@ApiForbiddenResponse({ description: 'Token lacks the pricing:admin scope.' })
@Scopes(Scope.ADMIN)
@Controller({ path: 'admin', version: '1' })
export class AdminController {
  constructor(
    private readonly adminService: AdminService,
    private readonly authService: AuthService,
  ) {}

  @Post('models/:provider/:name/price')
  @ApiOperation({ summary: 'Manually set a model price component (source=manual).' })
  @ApiParam({ name: 'provider', example: 'openai' })
  @ApiParam({ name: 'name', example: 'dall-e-3' })
  @ApiOkResponse({ type: PriceComponentDto })
  upsertPrice(
    @Param('provider') provider: string,
    @Param('name') name: string,
    @Body() dto: UpsertPriceDto,
    @CurrentClient() client: AuthPrincipal,
  ): Promise<PriceComponentDto> {
    return this.adminService.upsertPrice(`${provider}/${name}`, dto, client.clientId);
  }

  @Patch('providers/:id')
  @ApiOperation({ summary: 'Update a provider refresh mode / cron / pricing URL.' })
  @ApiParam({ name: 'id', example: 'openai' })
  @ApiOkResponse({ type: ProviderDto })
  updateProvider(@Param('id') id: string, @Body() dto: UpdateProviderDto): Promise<ProviderDto> {
    return this.adminService.updateProvider(id, dto);
  }

  @Post('providers/:id/scrape')
  @ApiOperation({ summary: 'Trigger an immediate scrape for a provider.' })
  @ApiParam({ name: 'id', example: 'openai' })
  @ApiOkResponse({ type: ScrapeRunDto })
  triggerScrape(@Param('id') id: string): Promise<ScrapeRunDto> {
    return this.adminService.triggerScrape(id);
  }

  @Post('clients')
  @ApiOperation({ summary: 'Create a new OAuth client (secret returned once).' })
  @ApiCreatedResponse({ type: CreatedClientDto })
  createClient(@Body() dto: CreateClientDto): Promise<CreatedClientDto> {
    return this.authService.createClient(dto.name, dto.scopes);
  }
}
