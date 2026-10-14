import { LiteGenClient, GenerationStatus, Model3dAssetKind } from "../src";

const client = new LiteGenClient({
  baseUrl: process.env.LITEGEN_BASE_URL ?? "http://localhost:4000",
  apiKey: process.env.LITEGEN_API_KEY,
});

// What can this model do? Its schema lists the formats it can deliver, the
// reference-image roles it takes and whether it reads a prompt.
const schema = await client.models.get("fal/hyper3d-rodin");
console.log("Rodin delivers:", (schema.params?.output_formats as { enum_values?: string[] })?.enum_values);

// Text-to-3D. `output_formats` is a promise: a completed job carries a mesh in
// every format listed, or it fails. Rodin makes one container per provider
// job, but litegen derives obj/stl/ply from the glb, so this is still one job.
const job = client.models3d.generate({
  model: "fal/hyper3d-rodin",
  prompt: "a weathered bronze statue of a fox",
  output_formats: ["glb", "obj", "stl"],
});

// Stream progress (each update carries the unified 0–100 progress)…
let final = await job.submitted;
for await (const update of job) {
  final = update;
  console.log(`${update.status} — ${update.progress}%`);
}

if (final.status === GenerationStatus.Completed) {
  for (const asset of final.assets ?? []) {
    console.log(`${asset.kind} ${asset.format}: ${asset.url}`);
  }
  const glb = final.assets?.find((a) => a.kind === Model3dAssetKind.Mesh && a.format === "glb");
  console.log("Preview this one in a glTF viewer:", glb?.url);
} else {
  console.error(`Generation ${final.status}:`, final.error);
}

// Image-to-3D: the picture goes in `reference_images` under the `init` role.
// TRELLIS takes no prompt at all, so none is sent. `strict: false` makes the
// server drop a param the model does not support instead of rejecting the
// request — and `droppedParams` says which.
const fromImage = client.models3d.generate({
  model: "fal/trellis",
  reference_images: [{ type: "url", value: "https://example.com/chair.png", role: "init" }],
  output_formats: ["glb", "ply"],
  strict: false,
  pbr: true,
});
console.log("Dropped by lax mode:", await fromImage.droppedParams); // ["pbr"]
const mesh = await fromImage; // polls up to 30 minutes by default
console.log(mesh.status, mesh.assets?.map((a) => `${a.kind}:${a.format}`));

// A job you no longer want can be cancelled while it is pending or processing:
//
//   await client.generations.cancel((await job.submitted).id);
