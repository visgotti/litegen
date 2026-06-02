import { ApiProperty } from '@nestjs/swagger';
import { ScrapeStatus } from '../../../common/enums';
import { ScrapeRunEntity } from '../../../entities';

/** Summary of a triggered scrape run. */
export class ScrapeRunDto {
  @ApiProperty()
  id!: string;

  @ApiProperty({ example: 'openai' })
  providerId!: string;

  @ApiProperty({ enum: ScrapeStatus })
  status!: ScrapeStatus;

  @ApiProperty({ example: 2 })
  componentsUpdated!: number;

  @ApiProperty({ example: 2 })
  componentsSeen!: number;

  @ApiProperty({ nullable: true })
  durationMs!: number | null;

  @ApiProperty({ nullable: true })
  error!: string | null;

  static fromEntity(run: ScrapeRunEntity): ScrapeRunDto {
    const dto = new ScrapeRunDto();
    dto.id = run.id;
    dto.providerId = run.providerId;
    dto.status = run.status;
    dto.componentsUpdated = run.componentsUpdated;
    dto.componentsSeen = run.componentsSeen;
    dto.durationMs = run.durationMs;
    dto.error = run.error;
    return dto;
  }
}
