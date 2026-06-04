# Model comparison images

Result images for the landing page's **Model comparison** section
(`<ModelComparison>`), one per `provider:model` for each evaluation prompt.

## Adding a comparison

1. Create a folder per prompt and drop the result images in it, e.g.:

   ```
   public/comparisons/red-panda/openai.png
   public/comparisons/red-panda/stability.png
   public/comparisons/red-panda/google.png
   ```

   Square-ish images look best; any aspect ratio works (they're contained, not
   cropped). Keep them reasonably sized (≈768–1024px) so the page stays light.

2. Add an entry to `src/config/comparisons.ts`. The `image` field is the path
   **under this folder**:

   ```ts
   {
     id: 'red-panda',
     label: 'Red panda',
     prompt: 'a red panda coding at a desk, cinematic lighting',
     results: [
       { provider: 'openai',    model: 'dall-e-3', image: 'red-panda/openai.png' },
       { provider: 'stability', model: 'sdxl',     image: 'red-panda/stability.png' },
       { provider: 'google',    model: 'imagen-3', image: 'red-panda/google.png' },
     ],
   },
   ```

The section is **hidden** automatically while `COMPARISONS` is empty, so it's
safe to leave this folder empty until you have real evaluation outputs.
