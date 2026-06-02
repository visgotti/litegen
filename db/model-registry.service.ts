import { Injectable, Logger, OnModuleInit } from '@nestjs/common';
import { InjectRepository } from '@nestjs/typeorm';
import { Repository } from 'typeorm';
import * as path from 'path';
import {
  DEFAULT_GENERATION_MODEL_ID,
  GenerationModelDefinition,
  GenerationModelRuntimeConfig,
  GenerationModelRunner,
  GenerationModelSummary,
  ImageSize,
} from '@pixel-genie/types';
import { AiModel, MediaType } from '../../db/modules/ai-model/ai-model.entity';

import * as fs from 'fs';

/**
 * Map a model-info.json `defaultSettings.width` (or height) to the
 * correct `ImageSize` enum string the worker's `parseSize()` expects.
 * Falls back to `"512x512"` if the value doesn't match any known size.
 */
function resolveRecommendedImageSize(widthOrSize: number | string | undefined): ImageSize {
  if (!widthOrSize) return ImageSize.EXTRA_LARGE; // safe default

  // Already a proper "NxN" string
  if (typeof widthOrSize === 'string' && widthOrSize.includes('x')) {
    return widthOrSize as ImageSize;
  }

  const dim = typeof widthOrSize === 'string' ? parseInt(widthOrSize, 10) : widthOrSize;
  const sizeMap: Record<number, ImageSize> = {
    32: ImageSize.TINY,
    64: ImageSize.SMALL,
    128: ImageSize.MEDIUM,
    256: ImageSize.LARGE,
    512: ImageSize.EXTRA_LARGE,
    1024: ImageSize.MASSIVE,
  };
  return sizeMap[dim] ?? ImageSize.EXTRA_LARGE;
}

@Injectable()
export class ModelRegistryService implements OnModuleInit {
  private readonly logger = new Logger(ModelRegistryService.name);
  /** Active models only — used for generation routing */
  private models: GenerationModelDefinition[] = [];
  /** All models including disabled/deleted — used for lookups and bootstrap */
  private allModels: GenerationModelDefinition[] = [];

  constructor(
    @InjectRepository(AiModel)
    private readonly aiModelRepository: Repository<AiModel>,
  ) {
    // Empty until onModuleInit — models come exclusively from disk → DB.
    this.models = [];
    this.allModels = [];
  }

  async onModuleInit() {
    await this.syncModelsFromDisk();
    await this.refreshModels();
  }

  /**
   * Single source of truth: read each ai/{model}/model-info.json from disk.
   * - enabled → upsert into DB (and update config if already exists)
   * - disabled → deactivate in DB
   * The DB is the runtime store; model-info.json is the authoritative config.
   */
  private async syncModelsFromDisk() {
    const syncedModelIds: string[] = [];

    try {
      const aiDir = path.join(process.cwd(), 'ai');
      if (!fs.existsSync(aiDir)) {
        this.logger.warn(`ai/ directory not found at ${aiDir}, no local models to sync`);
        return;
      }

      const modelDirs = fs.readdirSync(aiDir).filter(
        d => fs.statSync(path.join(aiDir, d)).isDirectory(),
      );

      for (const dir of modelDirs) {
        const infoPath = path.join(aiDir, dir, 'model-info.json');
        if (!fs.existsSync(infoPath)) continue;

        try {
          const info = JSON.parse(fs.readFileSync(infoPath, 'utf-8'));
          const modelId = info.slug || dir.replace('-gen', '');

          if (info.enabled) {
            await this.upsertModelFromInfo(info, dir);
            syncedModelIds.push(modelId);
          } else {
            // Soft-delete disabled models so historical FKs remain valid
            const existingModel = await this.aiModelRepository.findOne({ where: { id: modelId } });
            if (existingModel && existingModel.isActive) {
              existingModel.softDelete(); // sets isActive=false + deletedAt
              await this.aiModelRepository.save(existingModel);
              this.logger.log(`Soft-deleted model ${modelId} (disabled in model-info.json)`);
            }
          }
        } catch (e) {
          this.logger.error(`Failed to parse model info for ${dir}: ${e.message}`);
        }
      }
    } catch (error) {
      this.logger.error(`Failed to sync models from disk: ${error.message}`);
    }

    // Deactivate orphaned local models not present in any model-info.json.
    // This prevents duplicates (e.g. "sd15" vs "sd_1_5") from old provider syncs.
    if (syncedModelIds.length > 0) {
      try {
        const allLocalModels = await this.aiModelRepository.find({
          where: { isExternal: false, mediaType: MediaType.IMAGE, isActive: true },
        });

        for (const model of allLocalModels) {
          if (!syncedModelIds.includes(model.id)) {
            model.softDelete();
            await this.aiModelRepository.save(model);
            this.logger.log(`Deactivated orphaned local model ${model.id} (not in any model-info.json)`);
          }
        }
      } catch (error) {
        this.logger.error(`Failed to clean up orphaned models: ${error.message}`);
      }
    }
  }

  private async upsertModelFromInfo(info: any, dirName: string) {
    const modelId = info.slug || dirName.replace('-gen', '');

    const modelConfig = {
      runner: {
        type: GenerationModelRunner.PYTHON_SCRIPT,
        scriptPath: path.join(process.cwd(), 'ai', dirName, 'generate_image.py'),
        pythonPath: process.env.PYTHON_PATH || 'python3',
      },
      capabilities: {
        supportsTextToImage: info.capabilities?.textToImage ?? true,
        supportsImageToImage: info.capabilities?.imageToImage ?? false,
        maxImages: 1,
        recommendedImageSize: resolveRecommendedImageSize(info.defaultSettings?.width),
        recommendedStyles: [],
      },
      metadata: {
        requirements: info.requirements,
        huggingFaceId: info.huggingFaceId,
      },
    };

    let model = await this.aiModelRepository.findOne({ where: { id: modelId } });

    if (!model) {
      model = this.aiModelRepository.create({
        id: modelId,
        displayName: info.displayName || modelId,
        description: info.description,
        isExternal: false,
        isActive: true,
        isDefault: false,
        mediaType: MediaType.IMAGE,
        config: modelConfig,
      });
      await this.aiModelRepository.save(model);
      this.logger.log(`Synced new model ${modelId} from disk (${dirName})`);
    } else {
      // Always update config from disk so model-info.json stays authoritative
      model.displayName = info.displayName || model.displayName;
      model.description = info.description || model.description;
      model.isActive = true;
      model.config = modelConfig;
      await this.aiModelRepository.save(model);
      this.logger.log(`Updated model ${modelId} from disk (${dirName})`);
    }
  }

  /**
   * Load models from DB into memory.
   * - `this.models` = active models only (for generation routing)
   * - `this.allModels` = every model including disabled/deleted (for bootstrap + history lookups)
   */
  public async refreshModels() {
    const configuredDefault = process.env.DEFAULT_GENERATION_MODEL_ID ?? DEFAULT_GENERATION_MODEL_ID;

    try {
      // Load ALL image models (active + inactive/deleted) for historical lookups & bootstrap
      const everyDbModel = await this.aiModelRepository.find({
        relations: ['provider'],
        where: { mediaType: MediaType.IMAGE },
      });

      this.allModels = everyDbModel.map(m => ({
        ...this.mapDbModelToDefinition(m),
        isDefault: false, // default only applies to active set
        metadata: {
          ...this.mapDbModelToDefinition(m).metadata,
          isActive: m.isActive,
          deletedAt: m.deletedAt ?? null,
        },
      }));

      // Active models for generation routing
      const activeDbModels = everyDbModel.filter(m => m.isActive);

      if (activeDbModels.length > 0) {
        this.models = activeDbModels.map(m => this.mapDbModelToDefinition(m)).map(model => ({
          ...model,
          isDefault: model.id === configuredDefault,
        }));

        // If none matched the configured default, mark the first model as default
        if (!this.models.some(m => m.isDefault)) {
          this.models[0].isDefault = true;
        }

        this.logger.log(
          `Loaded ${this.models.length} active model(s), ${this.allModels.length} total from DB: ${this.models.map(m => `${m.id} (${m.displayName})`).join(', ')}`,
        );
      } else {
        this.logger.warn('No active models found in DB. Client will see an empty model list.');
        this.models = [];
      }
    } catch (error) {
      this.logger.error(`Failed to load models from DB: ${error.message}`, error.stack);
    }
  }

  private mapDbModelToDefinition(dbModel: AiModel): GenerationModelDefinition {
    return {
      id: dbModel.id,
      displayName: dbModel.displayName,
      description: dbModel.description || '',
      isDefault: dbModel.isDefault,
      tags: dbModel.tags || [],
      runner: {
        type: dbModel.isExternal
          ? 'EXTERNAL_API' as any
          : (dbModel.config?.runner?.type || GenerationModelRunner.PYTHON_SCRIPT),
        ...dbModel.config?.runner,
      },
      capabilities: {
        supportsTextToImage: true,
        supportsImageToImage: false,
        maxImages: 1,
        recommendedImageSize: ImageSize.EXTRA_LARGE,
        recommendedStyles: [],
        ...dbModel.config?.capabilities,
      },
      metadata: {
        provider: dbModel.provider?.name,
        isExternal: dbModel.isExternal,
        ...dbModel.config?.metadata,
      },
    };
  }

  public getModels(): GenerationModelDefinition[] {
    return this.models;
  }

  /** Returns only active models for the models endpoint / dropdown.
   *  Deduplicates by displayName to prevent showing the same model twice. */
  public getPublicModels(): GenerationModelSummary[] {
    const seen = new Map<string, GenerationModelSummary>();

    for (const model of this.models) {
      const entry: GenerationModelSummary = {
        id: model.id,
        displayName: model.displayName,
        description: model.description,
        isDefault: model.isDefault,
        tags: model.tags,
        capabilities: model.capabilities,
      };

      const existing = seen.get(model.displayName);
      if (!existing || (entry.isDefault && !existing.isDefault)) {
        seen.set(model.displayName, entry);
      }
    }

    return Array.from(seen.values());
  }

  /** Returns ALL models (active + disabled/deleted) with a `disabled` flag for bootstrap.
   *  Deduplicates by displayName — if two models share a name, the active one wins;
   *  among equals the one whose id matches a disk slug wins. */
  public getAllPublicModels(): (GenerationModelSummary & { disabled: boolean })[] {
    return this.getAllPublicModelsForProviders();
  }

  /**
   * Returns ALL models filtered to only include those whose provider is in the given set.
   * Models with no known provider are always included (e.g. local/self-hosted).
   * If no healthyProviders set is given, all models are returned (backwards-compatible).
   */
  public getAllPublicModelsForProviders(healthyProviders?: Set<string>): (GenerationModelSummary & { disabled: boolean })[] {
    const seen = new Map<string, GenerationModelSummary & { disabled: boolean }>();

    for (const model of this.allModels) {
      // Filter by healthy provider if set is provided
      if (healthyProviders) {
        const providerName = model.metadata?.provider;
        const isExternal = model.metadata?.isExternal === true;

        if (providerName && !healthyProviders.has(providerName)) {
          continue; // skip models from unhealthy providers
        }
        // External models without a provider relation are orphaned — exclude them
        // unless at least one provider is healthy (shouldn't happen normally)
        if (isExternal && !providerName) {
          continue;
        }
      }

      const isActive = model.metadata?.isActive === true;
      const entry: GenerationModelSummary & { disabled: boolean } = {
        id: model.id,
        displayName: model.displayName,
        description: model.description,
        isDefault: isActive ? (this.models.find(m => m.id === model.id)?.isDefault ?? false) : false,
        tags: model.tags,
        capabilities: model.capabilities,
        disabled: !isActive,
      };

      const existing = seen.get(model.displayName);
      if (!existing) {
        seen.set(model.displayName, entry);
      } else {
        // Prefer active over disabled, then prefer the one that's a default
        const existingActive = !existing.disabled;
        const newActive = !entry.disabled;
        if ((!existingActive && newActive) || (entry.isDefault && !existing.isDefault)) {
          seen.set(model.displayName, entry);
        }
      }
    }

    return Array.from(seen.values());
  }

  /**
   * Look up a model by ID. First checks active models, then falls back to allModels
   * (which includes disabled/deleted). Returns the default model only when no ID is supplied.
   */
  public getModelById(id?: string): GenerationModelDefinition {
    if (!id) {
      return this.getDefaultModel();
    }

    // First try active models
    const activeModel = this.models.find(item => item.id === id);
    if (activeModel) {
      return activeModel;
    }

    // Then try ALL models (including disabled/deleted) for historical lookups
    const anyModel = this.allModels.find(item => item.id === id);
    if (anyModel) {
      this.logger.debug(`Model ${id} found but is inactive/deleted — returning for historical lookup.`);
      return anyModel;
    }

    this.logger.warn(`Requested model ${id} not found in any registry. Falling back to default model.`);
    return this.getDefaultModel();
  }

  public getDefaultModel(): GenerationModelDefinition {
    const found = this.models.find(model => model.isDefault) ?? this.models[0];
    if (!found) {
      // Absolute last-resort mock so the server doesn't crash before DB is ready
      this.logger.error('No models loaded — returning mock fallback. This should not happen in production.');
      return {
        id: 'sdxl',
        displayName: 'Mock (no models loaded)',
        description: 'Temporary fallback — no models found in DB.',
        isDefault: true,
        tags: [],
        runner: { type: GenerationModelRunner.MOCK },
        capabilities: {
          supportsTextToImage: true,
          supportsImageToImage: false,
          maxImages: 1,
          recommendedImageSize: ImageSize.MEDIUM,
          recommendedStyles: [],
        },
      };
    }
    return found;
  }

  public getDefaultModelId(): string {
    return this.getDefaultModel().id;
  }

  /**
   * Deactivate a model by ID, removing it from the active model list.
   * Called when a provider reports a pricing error at runtime — the model
   * is soft-deleted so it no longer appears in the client UI.
   *
   * @returns true if the model was found and deactivated, false if not found
   */
  public async deactivateModel(modelId: string): Promise<boolean> {
    const dbModel = await this.aiModelRepository.findOne({ where: { id: modelId } });
    if (!dbModel) {
      this.logger.warn(`deactivateModel: model ${modelId} not found in DB`);
      return false;
    }
    if (!dbModel.isActive) {
      this.logger.debug(`deactivateModel: model ${modelId} already inactive`);
      return true;
    }

    dbModel.isActive = false;
    dbModel.deletedAt = new Date();
    await this.aiModelRepository.save(dbModel);
    this.logger.warn(`Deactivated model ${modelId} (pricing error / no cost data)`);

    // Refresh in-memory cache so the change takes effect immediately
    await this.refreshModels();
    return true;
  }

  public getRuntimeConfig(id?: string): GenerationModelRuntimeConfig {
    const model = this.getModelById(id);
    return {
      id: model.id,
      runner: model.runner,
      metadata: model.metadata,
    };
  }
}
