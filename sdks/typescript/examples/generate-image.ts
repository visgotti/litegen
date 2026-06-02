import { LiteGenClient } from "../src";

const client = new LiteGenClient({
  baseUrl: process.env.LITEGEN_BASE_URL ?? "http://localhost:4000",
  apiKey: process.env.LITEGEN_API_KEY,
});

const result = await client.images.generate({
  prompt: "a serene mountain landscape at sunset, oil painting",
  model: "openai/dall-e-3",
  size: "1024x1024",
  quality: "hd",
});

console.log("Image URL:", result.data[0]?.url);
console.log("Provider:", result.provider);
if (result.usage) {
  console.log("Cost USD:", result.usage.cost_usd);
}
