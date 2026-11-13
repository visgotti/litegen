// node --test scripts/mesh-verify/usd-variants.test.mjs
//
// The pure rules of mesh-gaps item 6's harness checks (USDC, several clips,
// unlit), shared by verify.mjs: which manifest cases are USD variants, how an
// `Animation` variant set is read from `usdcat`'s text and selected through a
// session layer, what `usdchecker` reports per variant, what three.js must
// list for a multi-clip file, the usdcat checks, and the two-light unlit
// verdict on pictures.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  UNLIT_LIMITS,
  USDZ_ALL,
  USDZ_CRATE,
  USDZ_CRATE_FIRST,
  USD_VARIANTS,
  animationVariants,
  baseColourMatch,
  clipRanges,
  clipVariantsVerdict,
  crateRooted,
  clipSampleTimes,
  coverageMask,
  crateEmptyArrays,
  groupNames,
  lightInvariance,
  lightResponse,
  meshesOf,
  missingVariantsReason,
  pixelsDiffer,
  pureMasks,
  textDiff,
  threeClipsVerdict,
  unlitVerdict,
  usdVariantCase,
  usdcatVerdict,
  usdcheckerResults,
  usdcheckerVariantVerdict,
  variantOver,
} from './usd-variants.mjs';

// `usdcat` of a hand-written two-clip file (Apple's layout): the text a variant
// set is read from. Printed by Apple USD Tools 0.25.2 (scratch/h/text_a.txt).
const TWO_CLIPS = `#usda 1.0
(
    defaultPrim = "Root"
    endTimeCode = 30
    timeCodesPerSecond = 30
)

def Xform "Root" (
    variants = {
        string Animation = "bend"
    }
    prepend variantSets = "Animation"
)
{
    def SkelRoot "Rig"
    {
        def Skeleton "Skel"
        {
            def SkelAnimation "bend"
            {
            }
            def SkelAnimation "twist"
            {
            }
        }
    }
    variantSet "Animation" = {
        "bend" {
            over "Rig"
            {
                over "Skel"
                {
                    rel skel:animationSource = </Root/Rig/Skel/bend>
                }
            }

        }
        "twist" {
            over "Rig"
            {
                over "Skel"
                {
                    rel skel:animationSource = </Root/Rig/Skel/twist>
                }
            }

        }
    }
}
`;

test('the manifest names the USDZ variants, and each is a usdz case', () => {
  assert.deepEqual(USD_VARIANTS, [USDZ_ALL, USDZ_CRATE, USDZ_CRATE_FIRST]);
  assert.equal(USDZ_ALL, 'usdz-all');
  assert.equal(USDZ_CRATE, 'usdz-crate');
  const raw = { fixture: 'clips', from: 'scene', to: 'usdz-crate', primary: 'x/model.usdz' };
  assert.deepEqual(usdVariantCase(raw), { ...raw, to: 'usdz', variant: 'usdz-crate' });
  assert.equal(usdVariantCase({ ...raw, to: 'usdz-all' }).variant, 'usdz-all');
  assert.equal(usdVariantCase({ ...raw, to: 'usdz' }), null, 'the plain usdz is not a variant');
  assert.equal(usdVariantCase({ ...raw, to: 'dae-keep' }), null, 'dae-keep is another variant, item 8');
});

test('an Animation variant set is read from usdcat text: the root, the selection and the names in order', () => {
  assert.deepEqual(animationVariants(TWO_CLIPS), { root: 'Root', selected: 'bend', names: ['bend', 'twist'] });
});

test('a file with no Animation variant set, or another one, has no variants', () => {
  assert.deepEqual(animationVariants('#usda 1.0\n(\n    defaultPrim = "Root"\n)\n\ndef Xform "Root"\n{\n}\n'), { root: 'Root', selected: null, names: [] });
  const look = TWO_CLIPS.replaceAll('Animation', 'look');
  assert.deepEqual(animationVariants(look).names, [], 'a look variant set is not the clips convention');
  assert.deepEqual(animationVariants('').names, []);
});

test('a name or a comment that looks like a variant does not become one', () => {
  const tricky = TWO_CLIPS.replace(
    '        "twist" {',
    '        # "ghost" { in a comment\n        "twist" {',
  ).replace('rel skel:animationSource = </Root/Rig/Skel/twist>', 'rel skel:animationSource = </Root/Rig/Skel/twist>\n                    string doc = "\\"phantom\\" { x }"');
  assert.deepEqual(animationVariants(tricky).names, ['bend', 'twist']);
});

test('the selection is a session-layer over of the default prim, written as the research recipe', () => {
  assert.equal(variantOver('Root', 'run'), 'over "Root" (\n    variants = {\n        string Animation = "run"\n    }\n)\n{\n}\n');
  assert.throws(() => variantOver('Root', 'bad"name'), /variant name/);
  assert.throws(() => variantOver('Ro"ot', 'run'), /prim name/);
});

test('usdchecker reports each variant, with and without a selection, and a failure is a failed variant', () => {
  const out = `
Validation Result with no explicit variants set
Success!

Validation Result for Variants: Animation:bend on Prim: /Root
Success!

Validation Result for Variants: Animation:twist on Prim: /Root
Success!

Validation Result for Variants: Animation: on Prim: /Root
Success!
`;
  assert.deepEqual(usdcheckerResults(out), [
    { variant: null, prim: null, ok: true },
    { variant: 'bend', prim: '/Root', ok: true },
    { variant: 'twist', prim: '/Root', ok: true },
    { variant: '', prim: '/Root', ok: true },
  ]);
  const bad = out.replace('Animation:twist on Prim: /Root\nSuccess!', 'Animation:twist on Prim: /Root\nFailed!\nError: something');
  assert.deepEqual(usdcheckerResults(bad).find((r) => r.variant === 'twist'), { variant: 'twist', prim: '/Root', ok: false });
  assert.deepEqual(usdcheckerResults('Success!\n'), []);
});

test('usdchecker must report and accept every variant the file holds', () => {
  const all = [
    { variant: null, prim: null, ok: true },
    { variant: 'walk', prim: '/Root', ok: true },
    { variant: 'run', prim: '/Root', ok: true },
    { variant: 'wave', prim: '/Root', ok: true },
  ];
  assert.deepEqual(usdcheckerVariantVerdict(all, ['walk', 'run', 'wave']), { ok: true, reasons: [], reported: ['walk', 'run', 'wave'] });
  const missing = usdcheckerVariantVerdict(all.filter((r) => r.variant !== 'run'), ['walk', 'run', 'wave']);
  assert.equal(missing.ok, false);
  assert.match(missing.reasons.join(' '), /run/);
  assert.match(missing.reasons.join(' '), /did not report/);
  const failed = usdcheckerVariantVerdict(all.map((r) => (r.variant === 'wave' ? { ...r, ok: false } : r)), ['walk', 'run', 'wave']);
  assert.equal(failed.ok, false);
  assert.match(failed.reasons.join(' '), /wave.*failed/);
  assert.equal(usdcheckerVariantVerdict([], []).ok, true, 'no variants, nothing to report');
});

test('a clip is sampled at the fixed fractions of its own range, rounded to a millisecond', () => {
  assert.deepEqual(clipSampleTimes(0, 1), [0, 0.17, 0.36, 0.58, 0.83, 1]);
  assert.deepEqual(clipSampleTimes(0, 0.6), [0, 0.102, 0.216, 0.348, 0.498, 0.6]);
  assert.deepEqual(clipSampleTimes(0, 2), [0, 0.34, 0.72, 1.16, 1.66, 2]);
});

// A glTF with a skin over nodes 1-3 and a prop on node 0: walk moves the leg
// and the prop, run the leg alone, wave the hip only.
const GLTF = {
  nodes: [{ name: 'prop_node' }, { name: 'hip' }, { name: 'knee' }, { name: 'ankle' }],
  skins: [{ joints: [1, 2, 3] }],
  accessors: [
    { min: [0], max: [1] }, // 0: walk knee
    { min: [0], max: [1] }, // 1: walk prop
    { min: [0.5], max: [2] }, // 2: wave hip
    { min: [0], max: [0.6] }, // 3: run knee
    { min: [0], max: [0.6] }, // 4: run prop... (rigid, dropped by prepare, kept here to test)
    { min: [0], max: [3] }, // 5: morph weights
  ],
  animations: [
    {
      name: 'walk',
      samplers: [{ input: 0 }, { input: 1 }],
      channels: [{ sampler: 0, target: { node: 2, path: 'rotation' } }, { sampler: 1, target: { node: 0, path: 'translation' } }],
    },
    { name: 'run', samplers: [{ input: 3 }], channels: [{ sampler: 0, target: { node: 2, path: 'rotation' } }] },
    { name: 'wave', samplers: [{ input: 2 }], channels: [{ sampler: 0, target: { node: 1, path: 'rotation' } }] },
    { name: 'blend', samplers: [{ input: 5 }], channels: [{ sampler: 0, target: { node: 0, path: 'weights' } }] },
  ],
};

test('a clip range is read from the sampler inputs, and split into its skeletal and rigid ends', () => {
  assert.deepEqual(clipRanges(GLTF), [
    { name: 'walk', start: 0, end: 1, skeletalEnd: 1, rigidEnd: 1 },
    { name: 'run', start: 0, end: 0.6, skeletalEnd: 0.6, rigidEnd: null },
    { name: 'wave', start: 0.5, end: 2, skeletalEnd: 2, rigidEnd: null },
    { name: 'blend', start: 0, end: 3, skeletalEnd: null, rigidEnd: null },
  ]);
});

test('a clip whose sampler input has no min and max is refused, not guessed', () => {
  const bad = { ...GLTF, accessors: [{}, ...GLTF.accessors.slice(1)] };
  assert.throws(() => clipRanges(bad), /min and max/);
});

const WANT = {
  clips: [
    { name: 'walk', skeletalEnd: 1 },
    { name: 'run', skeletalEnd: 0.6 },
    { name: 'wave', skeletalEnd: 2 },
  ],
  rigidEnd: 1,
};
const LISTED = [
  { name: 'run', duration: 0.6 },
  { name: 'walk', duration: 1 },
  { name: 'wave', duration: 2 },
  { name: 'TransformAnimation', duration: 1 },
];

test('three.js lists every clip by name with its duration, and the first clip\'s rigid motion as TransformAnimation', () => {
  const v = threeClipsVerdict(LISTED, WANT, { crate: true });
  assert.deepEqual(v.reasons, []);
  assert.equal(v.ok, true);
  assert.deepEqual(v.durations, { judged: true });
});

test('a clip three.js does not list, an extra one, or a wrong duration fails the check', () => {
  const short = threeClipsVerdict(LISTED.filter((c) => c.name !== 'run'), WANT, { crate: true });
  assert.equal(short.ok, false);
  assert.match(short.reasons.join(' '), /run/);
  const extra = threeClipsVerdict([...LISTED, { name: 'ghost', duration: 1 }], WANT, { crate: true });
  assert.equal(extra.ok, false);
  assert.match(extra.reasons.join(' '), /ghost/);
  const slow = threeClipsVerdict(LISTED.map((c) => (c.name === 'wave' ? { ...c, duration: 1.5 } : c)), WANT, { crate: true });
  assert.equal(slow.ok, false);
  assert.match(slow.reasons.join(' '), /wave.*1\.5.*2/);
  const twice = threeClipsVerdict([...LISTED, { name: 'walk', duration: 1 }], WANT, { crate: true });
  assert.equal(twice.ok, false, 'a clip listed twice');
});

test('the rigid motion is a TransformAnimation when the first clip has it, and there is none when it has not', () => {
  const none = threeClipsVerdict(LISTED.filter((c) => c.name !== 'TransformAnimation'), WANT, { crate: true });
  assert.equal(none.ok, false);
  assert.match(none.reasons.join(' '), /TransformAnimation/);
  const unwanted = threeClipsVerdict(LISTED, { ...WANT, rigidEnd: null }, { crate: true });
  assert.equal(unwanted.ok, false);
  assert.match(unwanted.reasons.join(' '), /TransformAnimation/);
  const late = threeClipsVerdict(LISTED.map((c) => (c.name === 'TransformAnimation' ? { ...c, duration: 2 } : c)), WANT, { crate: true });
  assert.equal(late.ok, false);
  assert.match(late.reasons.join(' '), /TransformAnimation.*2.*1/);
});

test('a text root is judged on its names alone: three.js assumes 30 fps for USDA, so its durations are a cited skip', () => {
  const v = threeClipsVerdict(LISTED.map((c) => ({ ...c, duration: c.duration * 2 })), WANT, { crate: false });
  assert.equal(v.ok, true);
  assert.equal(v.durations.judged, false);
  assert.match(v.durations.skipped, /USDAParser\.js/);
  assert.match(v.durations.skipped, /30/);
  const missing = threeClipsVerdict(LISTED.filter((c) => c.name !== 'wave'), WANT, { crate: false });
  assert.equal(missing.ok, false);
});

test('usdcat must exit 0 and say nothing on stderr', () => {
  assert.deepEqual(usdcatVerdict({ code: 0, stdout: '#usda 1.0\n', stderr: '' }), { ok: true, reasons: [] });
  const warned = usdcatVerdict({ code: 0, stdout: '#usda 1.0\n', stderr: 'Warning: in _Foo at line 1 -- something\n' });
  assert.equal(warned.ok, false);
  assert.match(warned.reasons.join(' '), /warning/i);
  assert.match(warned.reasons.join(' '), /something/);
  const failed = usdcatVerdict({ code: 1, stdout: '', stderr: 'Failed to open "x.usdc"' });
  assert.equal(failed.ok, false);
  assert.match(failed.reasons.join(' '), /exit/);
  const empty = usdcatVerdict({ code: 0, stdout: '', stderr: '' });
  assert.equal(empty.ok, false, 'a file that prints nothing is not a layer');
});

test('two usdcat texts are equal, or the first differing line is named', () => {
  assert.deepEqual(textDiff('a\nb\nc\n', 'a\nb\nc\n'), { equal: true, lines: 3 });
  const d = textDiff('a\nb\nc\n', 'a\nB\nc\n');
  assert.equal(d.equal, false);
  assert.equal(d.line, 2);
  assert.equal(d.a, 'b');
  assert.equal(d.b, 'B');
  const longer = textDiff('a\nb\n', 'a\nb\nc\n');
  assert.equal(longer.equal, false);
  assert.equal(longer.line, 3);
  assert.equal(longer.a, undefined);
});

const rgba = (w, h, f) => {
  const out = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) out.set(f(x, y), (y * w + x) * 4);
  return out;
};

test('two pictures of one file are compared exactly, and what differs is counted', () => {
  const a = rgba(4, 4, (x, y) => (x < 2 ? [10, 20, 30, 255] : [0, 0, 0, 0]));
  assert.deepEqual(pixelsDiffer(a, a.slice(), 0), { pixels: 8, differ: 0, maxDelta: 0 });
  const b = a.slice();
  b.set([10, 20, 33, 255], 0);
  b.set([11, 20, 30, 255], 4);
  assert.deepEqual(pixelsDiffer(a, b, 0), { pixels: 8, differ: 2, maxDelta: 3 });
  assert.deepEqual(pixelsDiffer(a, b, 1), { pixels: 8, differ: 1, maxDelta: 3 }, 'a tolerance of 1 forgives the one-level change');
  const c = a.slice();
  c.set([0, 0, 0, 255], 12); // a pixel covered in one picture only (x 3, y 0)
  assert.equal(pixelsDiffer(a, c, 0).differ, 1);
  assert.throws(() => pixelsDiffer(a, a.slice(0, 8), 0), /size/);
});

test('the lit control and the unlit meshes are told apart by the names of their ancestors', () => {
  const chains = [
    { unlit: true, names: ['box_node', 'box', 'assembly'] },
    { unlit: true, names: ['mirrored_box', 'box', 'assembly'] },
    { unlit: false, names: ['wedge_node', 'wedge', 'assembly'] },
    { unlit: false, names: ['panel_node', 'panel', 'assembly'] },
  ];
  assert.deepEqual(groupNames(chains), { unlit: ['box_node', 'box', 'mirrored_box'], lit: ['wedge_node', 'wedge', 'panel_node', 'panel'] });
  const shared = [
    { unlit: true, names: ['a', 'both'] },
    { unlit: false, names: ['b', 'both'] },
  ];
  assert.deepEqual(groupNames(shared), { unlit: ['a'], lit: ['b'] }, 'a name on both sides says nothing');
});

test('a mesh prim is in a group when a segment of its path is one of the group\'s names', () => {
  const prims = [
    { type: 'Xform', path: '/Root/assembly' },
    { type: 'Xform', path: '/Root/assembly/box_node' },
    { type: 'Mesh', path: '/Root/assembly/box_node/box' },
    { type: 'Mesh', path: '/Root/assembly/wedge_node' },
    { type: 'Mesh', path: '/Root/assembly/mirrored_box/box' },
    { type: 'Mesh', path: '/Root/assembly/boxy' },
  ];
  assert.deepEqual(meshesOf(prims, ['box_node', 'mirrored_box']), ['/Root/assembly/box_node/box', '/Root/assembly/mirrored_box/box']);
  assert.deepEqual(meshesOf(prims, ['wedge_node']), ['/Root/assembly/wedge_node']);
  assert.deepEqual(meshesOf(prims, []), []);
});

test('coverage is a picture\'s alpha, and a pixel is pure when only one group covers it', () => {
  const unlit = rgba(4, 1, (x) => (x <= 1 ? [9, 9, 9, 255] : [0, 0, 0, 0])); // x 0, 1
  const lit = rgba(4, 1, (x) => (x >= 1 ? [9, 9, 9, 255] : [0, 0, 0, 0])); // x 1, 2, 3
  assert.deepEqual([...coverageMask(unlit)], [1, 1, 0, 0]);
  const pure = pureMasks(unlit, lit);
  assert.deepEqual([...pure.unlit], [1, 0, 0, 0], 'x 1 is covered by both: occlusion decides it, so it is judged for neither');
  assert.deepEqual([...pure.lit], [0, 0, 1, 1]);
});

test('the unlit pixels are the same at two light levels within a share of pixels, and the maximum change is measured', () => {
  const mask = Uint8Array.from([1, 1, 1, 1, 0]);
  const a = rgba(5, 1, () => [100, 100, 100, 255]);
  const same = lightInvariance(a, a.slice(), mask, 1);
  assert.deepEqual(same, { pixels: 4, differ: 0, ratio: 0, maxDelta: 0, meanDelta: 0 });
  const b = a.slice();
  b.set([103, 100, 100, 255], 0); // a highlight of 3 levels on one pixel
  b.set([200, 200, 200, 255], 16); // outside the mask: not counted
  const moved = lightInvariance(a, b, mask, 1);
  assert.equal(moved.pixels, 4);
  assert.equal(moved.differ, 1);
  assert.equal(moved.ratio, 0.25);
  assert.equal(moved.maxDelta, 3);
  assert.equal(moved.meanDelta, 0.75);
  assert.equal(lightInvariance(a, b, mask, 3).differ, 0);
});

test('the lit control must move with the light: the share of its pixels that changed past a threshold', () => {
  const mask = Uint8Array.from([1, 1, 1, 1]);
  const a = rgba(4, 1, () => [60, 20, 5, 255]);
  const b = rgba(4, 1, (x) => (x < 3 ? [240, 80, 20, 255] : [62, 20, 5, 255]));
  assert.deepEqual(lightResponse(a, b, mask, 8), { pixels: 4, moved: 3, ratio: 0.75, maxDelta: 180 });
  assert.deepEqual(lightResponse(a, a.slice(), mask, 8), { pixels: 4, moved: 0, ratio: 0, maxDelta: 0 });
});

test('the unlit pixels are compared with the base colour inside the mask alone, with a pixel\'s neighbours allowed', () => {
  const mask = Uint8Array.from([1, 1, 0, 0]);
  const want = rgba(4, 1, () => [200, 100, 50, 255]);
  const got = rgba(4, 1, (x) => (x < 2 ? [205, 100, 50, 255] : [0, 255, 0, 255])); // outside the mask: anything
  const ok = baseColourMatch(got, want, mask, 20, 4, 1);
  assert.deepEqual({ covered: ok.covered, differ: ok.differ, ratio: ok.ratio }, { covered: 2, differ: 0, ratio: 0 });
  const wrong = rgba(4, 1, () => [0, 0, 0, 255]);
  const bad = baseColourMatch(wrong, want, mask, 20, 4, 1);
  assert.equal(bad.differ, 2);
  assert.equal(bad.ratio, 1);
});

const GOOD = {
  invariance: { pixels: 900, differ: 0, ratio: 0, maxDelta: 0, meanDelta: 0 },
  baseColour: { covered: 900, differ: 10, ratio: 0.011 },
  response: { pixels: 600, moved: 590, ratio: 0.98, maxDelta: 200 },
};

test('the unlit verdict: invariant, the base colour, and a lit control that changed', () => {
  assert.deepEqual(unlitVerdict(GOOD, { judgeBase: true }), { ok: true, reasons: [] });
  assert.deepEqual(UNLIT_LIMITS.invariance, 0.01, 'within 1 %');
  const lit = unlitVerdict({ ...GOOD, invariance: { pixels: 900, differ: 900, ratio: 1, maxDelta: 120, meanDelta: 80 } }, { judgeBase: true });
  assert.equal(lit.ok, false);
  assert.match(lit.reasons.join(' '), /unlit/);
  assert.match(lit.reasons.join(' '), /100\.00 %/);
  assert.match(lit.reasons.join(' '), /120/);
  const dark = unlitVerdict({ ...GOOD, baseColour: { covered: 900, differ: 900, ratio: 1 } }, { judgeBase: true });
  assert.equal(dark.ok, false);
  assert.match(dark.reasons.join(' '), /base colour/);
  const skipped = unlitVerdict({ ...GOOD, baseColour: { covered: 900, differ: 900, ratio: 1 } }, { judgeBase: false });
  assert.equal(skipped.ok, true, 'a base colour measured but not judged here is recorded only');
  const still = unlitVerdict({ ...GOOD, response: { pixels: 600, moved: 0, ratio: 0, maxDelta: 0 } }, { judgeBase: true });
  assert.equal(still.ok, false);
  assert.match(still.reasons.join(' '), /lit control/);
});

test('with no unlit pixel, or no lit control, there is nothing to judge and the case fails', () => {
  const noUnlit = unlitVerdict({ ...GOOD, invariance: { pixels: 0, differ: 0, ratio: 0, maxDelta: 0, meanDelta: 0 } }, { judgeBase: true });
  assert.equal(noUnlit.ok, false);
  assert.match(noUnlit.reasons.join(' '), /no unlit pixel/);
  const noControl = unlitVerdict({ ...GOOD, response: { pixels: 0, moved: 0, ratio: 0, maxDelta: 0 } }, { judgeBase: true });
  assert.equal(noControl.ok, false);
  assert.match(noControl.reasons.join(' '), /no lit control/);
});

// ─── review fixes: variant order, crate-rooted cases ─────────────────────────

test('variants are compared as a set: usdcat prints them in name order, the glTF lists clips as authored', () => {
  // `clips()`: walk, run, wave; usdcat prints run, walk, wave.
  const found = { root: 'clips', selected: 'walk', names: ['run', 'walk', 'wave'] };
  assert.deepEqual(clipVariantsVerdict(found, ['walk', 'run', 'wave']), { ok: true, reasons: [] });
});

test('the authored selection must be the first clip, not the first printed variant', () => {
  const found = { root: 'clips', selected: 'run', names: ['run', 'walk', 'wave'] };
  const v = clipVariantsVerdict(found, ['walk', 'run', 'wave']);
  assert.equal(v.ok, false);
  assert.match(v.reasons[0], /selection is "run", not the first clip "walk"/);
});

test('a variant that is no clip of the expected scene, or a clip with no variant, is named', () => {
  const extra = clipVariantsVerdict({ root: 'clips', selected: 'walk', names: ['walk', 'run', 'jump'] }, ['walk', 'run']);
  assert.equal(extra.ok, false);
  assert.match(extra.reasons.join(' '), /jump/);
  const missing = clipVariantsVerdict({ root: 'clips', selected: 'walk', names: ['walk'] }, ['walk', 'run']);
  assert.equal(missing.ok, false);
  assert.match(missing.reasons.join(' '), /run/);
});

test('a crate-rooted USDZ with the default single clip is a case of its own, read as usdz', () => {
  assert.ok(USD_VARIANTS.includes(USDZ_CRATE_FIRST));
  assert.deepEqual(usdVariantCase({ fixture: 'clips', to: 'usdz-crate-first' }), { fixture: 'clips', to: 'usdz', variant: 'usdz-crate-first' });
  assert.equal(crateRooted({ to: 'usdc' }), true);
  assert.equal(crateRooted({ to: 'usdz', variant: USDZ_CRATE }), true);
  assert.equal(crateRooted({ to: 'usdz', variant: USDZ_CRATE_FIRST }), true);
  assert.equal(crateRooted({ to: 'usdz', variant: USDZ_ALL }), false);
  assert.equal(crateRooted({ to: 'usdz' }), false);
  assert.equal(crateRooted({ to: 'usda' }), false);
});

test('a crate holding an empty array is found from usdcat text, by the attribute that holds it', () => {
  const text = [
    'def SkelRoot "r"',
    '{',
    '    uniform token[] joints = []',
    '    uniform token[] blendShapes = ["a", "b"]',
    '    matrix4d[] restTransforms = []',
    '    float3[] points = [(0, 0, 0)]',
    '    float3[] extent.timeSamples = {',
    '        0: [],',
    '        12: [(0, 0, 0)],',
    '    }',
    '    string note = "[]"',
    '}',
  ].join('\n');
  const found = crateEmptyArrays(text);
  assert.deepEqual(found.names, ['joints', 'restTransforms', 'extent']);
  assert.equal(found.count, 3);
});

test('a layer with no empty array has none to report', () => {
  const found = crateEmptyArrays('def Xform "a"\n{\n    uniform token[] joints = ["x"]\n    int[] counts = [3, 3]\n}\n');
  assert.deepEqual(found, { count: 0, names: [] });
});

// ─── 2026-11-10 follow-up: a file that should have several clips ─────────────

test('a file whose expected scene has several clips but which has fewer than two variants is named, not passed over', () => {
  assert.equal(
    missingVariantsReason({ root: null, selected: null, names: [] }, ['walk', 'run']),
    'the expected scene has 2 clips (walk, run), but the file has no Animation variant set',
  );
  assert.equal(
    missingVariantsReason({ root: 'clips', selected: 'walk', names: ['walk'] }, ['walk', 'run', 'wave']),
    'the expected scene has 3 clips (walk, run, wave), but the file has only the variant "walk"',
  );
  // Nothing to say: one clip or none expected, or the variants are there.
  assert.equal(missingVariantsReason({ root: null, selected: null, names: [] }, ['walk']), null);
  assert.equal(missingVariantsReason({ root: null, selected: null, names: [] }, []), null);
  assert.equal(missingVariantsReason({ root: 'clips', selected: 'walk', names: ['walk', 'run'] }, ['walk', 'run']), null);
});
