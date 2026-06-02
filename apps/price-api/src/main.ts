import { Logger, VersioningType } from '@nestjs/common';
import { ConfigService } from '@nestjs/config';
import { NestFactory } from '@nestjs/core';
import { DocumentBuilder, SwaggerModule } from '@nestjs/swagger';
import helmet from 'helmet';
import { AppConfig } from './config/configuration';
import { AppModule } from './app.module';

async function bootstrap(): Promise<void> {
  const app = await NestFactory.create(AppModule, { bufferLogs: false });
  const configService = app.get(ConfigService<AppConfig, true>);
  const logger = new Logger('Bootstrap');

  // Security headers.
  app.use(helmet());

  // CORS allowlist (or '*' in dev).
  const corsOrigins = configService.get('corsOrigins', { infer: true });
  app.enableCors({ origin: corsOrigins === '*' ? true : corsOrigins });

  // URI versioning -> /v1/... (health + oauth are version-neutral).
  app.enableVersioning({ type: VersioningType.URI });

  app.enableShutdownHooks();

  // OpenAPI / Swagger docs.
  const swaggerConfig = new DocumentBuilder()
    .setTitle('LiteGen Price API')
    .setDescription(
      'Dynamic + curated model pricing for every provider LiteGen supports. ' +
        'Public read endpoints are rate-limited; admin writes require an OAuth2 bearer token.',
    )
    .setVersion('0.1')
    .addBearerAuth()
    .addServer('/', 'this server')
    .build();
  const document = SwaggerModule.createDocument(app, swaggerConfig);
  SwaggerModule.setup('docs', app, document, {
    swaggerOptions: { persistAuthorization: true },
  });

  const port = configService.get('port', { infer: true });
  await app.listen(port);
  logger.log(`price-api listening on :${port} — docs at /docs`);
}

void bootstrap();
