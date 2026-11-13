import { renderSeed } from './seed-render';

describe('renderSeed', () => {
  const out = renderSeed([
    { id: 'p/vid', provider: 'p', displayName: 'Vid', mediaType: 'video', baseCostUsd: 0.5 },
    { id: 'p/img', provider: 'p', displayName: "It's \"quoted\"", mediaType: 'image', baseCostUsd: 0.04 },
    { id: 'p/mesh', provider: 'p', displayName: 'Mesh', mediaType: 'model3d', baseCostUsd: 0.4 },
  ]);

  it('says it is generated from the catalog', () => {
    expect(out.startsWith("// GENERATED from litegen's models/*.yaml")).toBe(true);
  });

  it('lists the models sorted by id, names escaped', () => {
    expect([...out.matchAll(/id: "([^"]+)"/g)].map((m) => m[1])).toEqual(['p/img', 'p/mesh', 'p/vid']);
    expect(out).toContain('displayName: "It\'s \\"quoted\\""');
  });

  it("prices each model at its default request, in the unit its medium bills by", () => {
    const entry = (id: string) => out.slice(out.indexOf(`id: "${id}"`)).split('},\n')[0];
    expect(entry('p/img')).toContain('mediaType: MediaType.IMAGE');
    expect(entry('p/img')).toContain('unit: PriceUnit.PER_IMAGE, amountUsd: 0.04');
    expect(entry('p/vid')).toContain('mediaType: MediaType.VIDEO');
    expect(entry('p/vid')).toContain('unit: PriceUnit.PER_VIDEO, amountUsd: 0.5');
    expect(entry('p/mesh')).toContain('mediaType: MediaType.MODEL_3D');
    expect(entry('p/mesh')).toContain('unit: PriceUnit.PER_REQUEST, amountUsd: 0.4');
  });
});
