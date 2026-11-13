// What SceneKit's importers cannot read, as pure functions of a file's text
// (verify.mjs's SceneKit judges): the Collada animation clips it cannot play
// (D) and the USD texture tint it ignores (E), each a measurement cited.
//
// ─── Collada animation clips, as SceneKit plays them (render.swift's `clip`) ───
//
// SceneKit plays every animation in a file at once, so a file with several
// `<animation_clip>`s is played one clip at a time, the one three.js and the
// expected picture play. But SceneKit's importer binds animation data to its
// TARGET, not to the clip that instances it: when two clips animate the same
// node, every clip entry carries the last clip's keys over the first clip's
// duration and the others come out empty (measured on the Khronos Fox, whose
// three clips move the same bones: the "Survey" entry holds "Run"'s keys and
// "Walk"'s groups are empty). Such a file cannot be judged by SceneKit between
// its keys; its rest pose still is, and three.js plays each clip correctly.
export const SCENEKIT_SHARED_CLIPS =
  "SceneKit's Collada importer binds keys by target, so clips animating the same node replace each other's keys; animation judged by three.js";

// And SceneKit registers a clip under the clip's NAME, its id only when it has
// none (followups 2026-11-08, D). Measured 2026-11-08 on macOS 26.6.2 with
// SCNSceneSource.identifiersOfEntries / entryWithIdentifier(_, CAAnimation),
// loading the CesiumMilkTruck dae litegen writes (clip id "Wheels-2", name
// "Wheels"; nodes "Wheels" and "Wheels_001"; animations "Wheels-anim" and
// "Wheels_001-anim"; geometry "Wheels-mesh") and copies of it:
//   - id "Wheels-2" name "WheelsClip"  -> entry "WheelsClip"; "Wheels-2" is nil.
//   - id "WheelsClip" name "WheelsClip" -> entry "WheelsClip".
//   - a clip with no name, id "Zed-2"   -> entry "Zed-2".
//   - name "Walk.001"                   -> entry "Walk.001" (verbatim, no dot made `-`).
//   - name "Wheels" (a node's id), "Wheels_001" (a node's id), "Cesium_Milk_Truck"
//     (a node's id), "Wheels-mesh" (a geometry's id), "Wheels-anim" (an
//     animation's id): no entry for the clip at all (the file's own animation
//     entries stay), so entryWithIdentifier finds nothing for it, whichever of
//     its id and name is asked for.
//   - name "Scene" (the visual scene's id) and "Wheels.001" (a node's NAME, not
//     its id): an entry for the clip.
// So the key SceneKit knows a clip by is `name ?? id`, and a clip whose key is
// the id of a node, a geometry or an animation of the file has no entry and
// cannot be played by SceneKit. litegen's ids are as dae.rs documents
// (`Ids::make`: unique, so the clip of an animation named like a node is
// `Wheels-2`); it is SceneKit's one namespace for entries that loses the clip.

export const SCENEKIT_CLIP_SHADOWED =
  "SceneKit's Collada importer registers an <animation_clip> under its name (its id if it has none) and registers none whose key is the id of a node, a geometry or an animation of the file (measured 2026-11-08, macOS 26.6.2: a clip named like its node, \"Wheels\", has no entry, whatever its id), so it cannot play that clip; animation judged by three.js";

// The clips of a Collada file: { id, name, key, shadowed, nodes } each, `key`
// the identifier SceneKit registers it under, `shadowed` whether another entry
// of the file has that identifier (so SceneKit has none for the clip), and
// `nodes` the nodes the clip animates; and `shared`, whether two clips
// animate the same node.
export function colladaClips(text) {
  const targets = new Map();
  for (const m of text.matchAll(/<animation\b[^>]*\bid="([^"]+)"[^>]*>([\s\S]*?)<\/animation>/g)) {
    targets.set(m[1], [...m[2].matchAll(/<channel\b[^>]*\btarget="([^"/]+)/g)].map((x) => x[1]));
  }
  // The ids that SceneKit registers as entries of their own and a clip's key
  // collides with (measured, above).
  const entries = new Set([...text.matchAll(/<(?:node|geometry|animation)\b[^>]*?\bid="([^"]+)"/g)].map((x) => x[1]));
  const clips = [...text.matchAll(/<animation_clip\b([^>]*)>([\s\S]*?)<\/animation_clip>/g)].map((m) => {
    const id = (m[1].match(/\bid="([^"]+)"/) || [])[1];
    const name = (m[1].match(/\bname="([^"]+)"/) || [])[1];
    const key = name ?? id;
    return {
      id,
      name,
      key,
      shadowed: key !== undefined && entries.has(key),
      nodes: new Set([...m[2].matchAll(/<instance_animation\b[^>]*\burl="#([^"]+)"/g)].flatMap((x) => targets.get(x[1]) || [])),
    };
  });
  const seen = new Set();
  let shared = false;
  for (const c of clips) {
    for (const n of c.nodes) if (seen.has(n)) shared = true;
    for (const n of c.nodes) seen.add(n);
  }
  return { clips, shared };
}

// The clip a case plays in SceneKit, as the key SceneKit knows it by
// (render.swift's `clip`), and why SceneKit's animated frames are skipped, if
// they are: `want` is the fixture's first animation (the clip three.js plays),
// else the file's first clip.
export function sceneKitClip(text, want) {
  const { clips, shared } = colladaClips(text);
  const picked = clips.find((k) => k.name === want || k.id === want) || clips[0];
  let notPlayed = null;
  if (picked?.shadowed) notPlayed = SCENEKIT_CLIP_SHADOWED;
  else if (clips.length > 1 && shared) notPlayed = SCENEKIT_SHARED_CLIPS;
  return { clip: picked?.key ?? null, notPlayed };
}

// ─── USD texture tint, which SceneKit's importer ignores (followups 2026-11-08, E) ───
//
// UsdUVTexture's `inputs:scale` is "Scale to be applied to all components of
// the texture. value * scale + bias" (Apple USD Tools 0.25.2,
// /usr/lib/usd/usd/usdShaders/resources/shaders/shaderDefs.usda L196-L202,
// default (1, 1, 1, 1)), and litegen writes a base colour that tints a base-colour
// texture there (an OBJ's `Kd` beside `map_Kd`, MorphPrimitivesTest: red and
// green materials on one texture, scales (1, 0, 0, 1) and (0, 1, 0, 1); usd_write.rs).
// Both USD readers that follow it agree with the expected picture on a correct file:
// three.js USDLoader (page/render.js, the case's `albedo`) and usdrecord
// (`usdrecord:albedo`), 0.00 % in both. SceneKit draws the texture untinted:
// measured 2026-11-08 (macOS 26.6.2, render.swift albedo) on the case's own
// model.usda and on a copy with both scales set to (1, 1, 1, 1): the two PNGs are
// byte-identical, so SceneKit ignores `inputs:scale` (its picture is the grey
// checker whatever the file says, 100.00 % of 4x4 blocks apart from the tinted
// expected picture). So where the texture on a material's base colour has a scale
// whose colour channels are not (1, 1, 1), SceneKit's albedo is not a judge, and
// usdrecord's and three.js's are (`usdTextureTint`, below).
export const SCENEKIT_USD_TEXSCALE =
  "SceneKit's USD importer ignores UsdUVTexture's inputs:scale (shaderDefs.usda L196-L202: value * scale + bias), which tints a texture by the base colour (measured 2026-11-08: the case's model.usda and a copy with every scale (1, 1, 1, 1) draw byte-identical pictures); USD albedo judged by usdrecord and three.js USDLoader, which follow it";

// Whether a UsdUVTexture that feeds a UsdPreviewSurface's `inputs:diffuseColor`
// in a flattened USD stage (`usdcat --flatten`) has an `inputs:scale` whose
// colour channels are not (1, 1, 1): a tint of the albedo. A texture that feeds
// anything else is not one: litegen writes a normal map's (2, 2, 2, 1) scale
// (with bias (-1, -1, -1, 0)), and SceneKit's albedo of such a file, with its
// normal map, is 0 % apart (Khronos LightsPunctualLamp, 14 USD cases in item 5's
// camera set), so it stays judged. The alpha channel is not the albedo's.
// Prims are found by a scan that skips strings, asset paths, comments and
// parenthesised metadata (a `customData` dictionary), and keeps each prim's
// path, so two materials' textures of one name are told apart.
export function usdTextureTint(text) {
  const token = /"""[\s\S]*?"""|"(?:[^"\\\n]|\\.)*"|@[^@\n]*@|#[^\n]*|[(){}]|\b(?:def|over|class)\b(?:[ \t]+\w+)?[ \t]+"([^"]+)"/g;
  const stack = [];
  const prims = [];
  let paren = 0;
  let pending = null;
  for (const m of text.matchAll(token)) {
    if (m[1] !== undefined) {
      if (paren === 0) pending = m[1];
    } else if (m[0] === '(') paren++;
    else if (m[0] === ')') paren = Math.max(0, paren - 1);
    else if (m[0] === '{' && paren === 0) {
      stack.push({ path: `${stack.length ? stack[stack.length - 1].path : ''}/${pending ?? ''}`, open: m.index + 1 });
      pending = null;
    } else if (m[0] === '}' && paren === 0 && stack.length) {
      const p = stack.pop();
      prims.push({ path: p.path, body: text.slice(p.open, m.index) });
    }
  }
  const feeds = new Set();
  for (const p of prims) {
    if (!/\binfo:id = "UsdPreviewSurface"/.test(p.body)) continue;
    for (const c of p.body.matchAll(/\binputs:diffuseColor\.connect = <([^>.]+)\./g)) feeds.add(c[1]);
  }
  return prims.some((p) => {
    if (!feeds.has(p.path) || !/\binfo:id = "UsdUVTexture"/.test(p.body)) return false;
    const scale = p.body.match(/\bfloat4 inputs:scale = \(([^)]*)\)/);
    if (!scale) return false;
    const v = scale[1].split(',').map((x) => Number(x.trim()));
    return v.length === 4 && v.slice(0, 3).some((x) => x !== 1);
  });
}
