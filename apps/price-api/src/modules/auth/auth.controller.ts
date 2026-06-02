import { Body, Controller, HttpCode, Post, VERSION_NEUTRAL } from '@nestjs/common';
import { Throttle } from '@nestjs/throttler';
import { ApiOkResponse, ApiOperation, ApiTags } from '@nestjs/swagger';
import { Public } from '../../common/decorators/public.decorator';
import { AuthService } from './auth.service';
import { TokenRequestDto } from './dto/token-request.dto';
import { TokenResponseDto } from './dto/token-response.dto';

@ApiTags('auth')
@Controller({ path: 'oauth', version: VERSION_NEUTRAL })
export class AuthController {
  constructor(private readonly authService: AuthService) {}

  @Public()
  // Stricter rate limit than the global default to blunt credential stuffing.
  @Throttle({ default: { limit: 10, ttl: 60_000 } })
  @Post('token')
  @HttpCode(200)
  @ApiOperation({
    summary: 'OAuth2 client-credentials token endpoint.',
    description: 'Exchange a client id + secret for a short-lived JWT bearer token.',
  })
  @ApiOkResponse({ type: TokenResponseDto })
  token(@Body() dto: TokenRequestDto): Promise<TokenResponseDto> {
    return this.authService.issueToken(dto);
  }
}
