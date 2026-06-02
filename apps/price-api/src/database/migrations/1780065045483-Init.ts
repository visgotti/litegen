import { MigrationInterface, QueryRunner } from "typeorm";

export class Init1780065045483 implements MigrationInterface {
    name = 'Init1780065045483'

    public async up(queryRunner: QueryRunner): Promise<void> {
        // uuid PK defaults use uuid_generate_v4(); ensure the extension exists
        // so the migration is portable to a fresh Postgres instance.
        await queryRunner.query(`CREATE EXTENSION IF NOT EXISTS "uuid-ossp"`);
        await queryRunner.query(`CREATE TABLE "model_prices" ("id" uuid NOT NULL DEFAULT uuid_generate_v4(), "modelId" character varying(128) NOT NULL, "unit" character varying(24) NOT NULL, "unitAmount" numeric(12,4) NOT NULL DEFAULT '1', "amountUsd" numeric(14,6) NOT NULL, "currency" character varying(8) NOT NULL DEFAULT 'USD', "tier" jsonb, "tierKey" character varying(256) NOT NULL DEFAULT '*', "source" character varying(16) NOT NULL DEFAULT 'fallback', "freshness" character varying(16) NOT NULL DEFAULT 'fresh', "consecutiveFailures" integer NOT NULL DEFAULT '0', "lastUpdatedAt" TIMESTAMP WITH TIME ZONE NOT NULL, "lastAttemptAt" TIMESTAMP WITH TIME ZONE, "createdAt" TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(), "updatedAt" TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(), CONSTRAINT "PK_1409a57d11faf4496339c681f6e" PRIMARY KEY ("id"))`);
        await queryRunner.query(`CREATE INDEX "IDX_30a9d6da57dabc060dd5810770" ON "model_prices" ("modelId") `);
        await queryRunner.query(`CREATE UNIQUE INDEX "uq_model_price_component" ON "model_prices" ("modelId", "unit", "tierKey") `);
        await queryRunner.query(`CREATE TABLE "providers" ("id" character varying(64) NOT NULL, "displayName" character varying(128) NOT NULL, "mode" character varying(16) NOT NULL DEFAULT 'manual', "cronSchedule" character varying(64), "pricingUrl" character varying(512), "notes" text, "scraperImplemented" boolean NOT NULL DEFAULT false, "lastScrapedAt" TIMESTAMP WITH TIME ZONE, "lastScrapeStatus" character varying(16), "consecutiveFailures" integer NOT NULL DEFAULT '0', "createdAt" TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(), "updatedAt" TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(), CONSTRAINT "PK_af13fc2ebf382fe0dad2e4793aa" PRIMARY KEY ("id"))`);
        await queryRunner.query(`CREATE TABLE "models" ("id" character varying(128) NOT NULL, "providerId" character varying(64) NOT NULL, "displayName" character varying(128) NOT NULL, "mediaType" character varying(16) NOT NULL, "modeOverride" character varying(16), "active" boolean NOT NULL DEFAULT true, "createdAt" TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(), "updatedAt" TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(), CONSTRAINT "PK_ef9ed7160ea69013636466bf2d5" PRIMARY KEY ("id"))`);
        await queryRunner.query(`CREATE INDEX "IDX_2ce64b8d909a4385f26bcd363b" ON "models" ("providerId") `);
        await queryRunner.query(`CREATE TABLE "oauth_clients" ("id" uuid NOT NULL DEFAULT uuid_generate_v4(), "clientId" character varying(128) NOT NULL, "clientSecretHash" character varying(256) NOT NULL, "name" character varying(128) NOT NULL, "scopes" text NOT NULL, "active" boolean NOT NULL DEFAULT true, "lastUsedAt" TIMESTAMP WITH TIME ZONE, "createdAt" TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(), "updatedAt" TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(), CONSTRAINT "PK_c4759172d3431bae6f04e678e0d" PRIMARY KEY ("id"))`);
        await queryRunner.query(`CREATE UNIQUE INDEX "IDX_b0c094fe1ef0a6c4af8f2b10be" ON "oauth_clients" ("clientId") `);
        await queryRunner.query(`CREATE TABLE "price_history" ("id" uuid NOT NULL DEFAULT uuid_generate_v4(), "modelId" character varying(128) NOT NULL, "unit" character varying(24) NOT NULL, "unitAmount" numeric(12,4) NOT NULL DEFAULT '1', "amountUsd" numeric(14,6) NOT NULL, "currency" character varying(8) NOT NULL DEFAULT 'USD', "tier" jsonb, "tierKey" character varying(256) NOT NULL DEFAULT '*', "source" character varying(16) NOT NULL, "scrapeRunId" uuid, "note" character varying(256), "recordedAt" TIMESTAMP WITH TIME ZONE NOT NULL, CONSTRAINT "PK_e41e25472373d4b574b153229e9" PRIMARY KEY ("id"))`);
        await queryRunner.query(`CREATE INDEX "IDX_9d20fe44f6166be36e7fecf081" ON "price_history" ("recordedAt") `);
        await queryRunner.query(`CREATE INDEX "idx_price_history_model_recorded" ON "price_history" ("modelId", "recordedAt") `);
        await queryRunner.query(`CREATE TABLE "scrape_runs" ("id" uuid NOT NULL DEFAULT uuid_generate_v4(), "providerId" character varying(64) NOT NULL, "status" character varying(16) NOT NULL, "startedAt" TIMESTAMP WITH TIME ZONE NOT NULL, "finishedAt" TIMESTAMP WITH TIME ZONE, "durationMs" integer, "componentsUpdated" integer NOT NULL DEFAULT '0', "componentsSeen" integer NOT NULL DEFAULT '0', "sourceUrl" character varying(512), "error" text, CONSTRAINT "PK_7c271a723ce0a12f57edc6ae720" PRIMARY KEY ("id"))`);
        await queryRunner.query(`CREATE INDEX "idx_scrape_run_provider_started" ON "scrape_runs" ("providerId", "startedAt") `);
        await queryRunner.query(`ALTER TABLE "model_prices" ADD CONSTRAINT "FK_30a9d6da57dabc060dd58107708" FOREIGN KEY ("modelId") REFERENCES "models"("id") ON DELETE CASCADE ON UPDATE NO ACTION`);
        await queryRunner.query(`ALTER TABLE "models" ADD CONSTRAINT "FK_2ce64b8d909a4385f26bcd363b3" FOREIGN KEY ("providerId") REFERENCES "providers"("id") ON DELETE CASCADE ON UPDATE NO ACTION`);
    }

    public async down(queryRunner: QueryRunner): Promise<void> {
        await queryRunner.query(`ALTER TABLE "models" DROP CONSTRAINT "FK_2ce64b8d909a4385f26bcd363b3"`);
        await queryRunner.query(`ALTER TABLE "model_prices" DROP CONSTRAINT "FK_30a9d6da57dabc060dd58107708"`);
        await queryRunner.query(`DROP INDEX "public"."idx_scrape_run_provider_started"`);
        await queryRunner.query(`DROP TABLE "scrape_runs"`);
        await queryRunner.query(`DROP INDEX "public"."idx_price_history_model_recorded"`);
        await queryRunner.query(`DROP INDEX "public"."IDX_9d20fe44f6166be36e7fecf081"`);
        await queryRunner.query(`DROP TABLE "price_history"`);
        await queryRunner.query(`DROP INDEX "public"."IDX_b0c094fe1ef0a6c4af8f2b10be"`);
        await queryRunner.query(`DROP TABLE "oauth_clients"`);
        await queryRunner.query(`DROP INDEX "public"."IDX_2ce64b8d909a4385f26bcd363b"`);
        await queryRunner.query(`DROP TABLE "models"`);
        await queryRunner.query(`DROP TABLE "providers"`);
        await queryRunner.query(`DROP INDEX "public"."uq_model_price_component"`);
        await queryRunner.query(`DROP INDEX "public"."IDX_30a9d6da57dabc060dd5810770"`);
        await queryRunner.query(`DROP TABLE "model_prices"`);
    }

}
