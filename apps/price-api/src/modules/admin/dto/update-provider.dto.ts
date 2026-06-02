import { ApiPropertyOptional } from '@nestjs/swagger';
import { IsEnum, IsOptional, IsString } from 'class-validator';
import { ProviderMode } from '../../../common/enums';

/** Patch a provider's refresh configuration. */
export class UpdateProviderDto {
  @ApiPropertyOptional({ enum: ProviderMode })
  @IsOptional()
  @IsEnum(ProviderMode)
  mode?: ProviderMode;

  @ApiPropertyOptional({ example: '0 6 * * *', description: 'Empty string clears the schedule.' })
  @IsOptional()
  @IsString()
  cronSchedule?: string;

  @ApiPropertyOptional({ example: 'https://example.com/pricing' })
  @IsOptional()
  @IsString()
  pricingUrl?: string;

  @ApiPropertyOptional()
  @IsOptional()
  @IsString()
  notes?: string;
}
