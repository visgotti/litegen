// GENERATED from litegen's models/*.yaml by `npm run coverage:fix`: each
// model at its default request's price. Edit the catalog, not this file.
import { MediaType, PriceUnit } from '../../common/enums';
import { SeedModel } from '../types';

export const seed: SeedModel[] = [
  {
    id: "google/gemini-2.5-flash-image",
    displayName: "Gemini 2.5 Flash (image)",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.039 }],
  },
  {
    id: "google/gemini-3-pro-image",
    displayName: "Gemini 3 Pro (image)",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.134 }],
  },
  {
    id: "google/gemini-3.1-flash-image",
    displayName: "Gemini 3.1 Flash Image (Nano Banana 2)",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.067 }],
  },
  {
    id: "google/gemini-3.1-flash-lite-image",
    displayName: "Gemini 3.1 Flash Lite Image (Nano Banana 2 Lite)",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.0336 }],
  },
  {
    id: "google/gemini-nano-banana-2.1",
    displayName: "Gemini Nano Banana 2.1",
    mediaType: MediaType.IMAGE,
    prices: [{ unit: PriceUnit.PER_IMAGE, amountUsd: 0.0336 }],
  },
];
