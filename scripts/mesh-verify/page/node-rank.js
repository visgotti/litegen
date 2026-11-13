// How the harness names a glTF node that has no name (item 5's Ruling R25,
// amended 2026-11-08): by its depth-first rank in the default scene, not by
// its index in the file.
//
// litegen's glTF reader numbers a scene's nodes depth-first from the roots
// (litegen-core/src/mesh/glb.rs `nodes`, L2794-L2864: an explicit stack, the
// roots and then each node's children pushed in reverse so that they pop in
// file order; a node's number is its place in that walk), and its writers
// name an unnamed node after its place in the scene they were given: FBX
// `node{i}` (fbx_write.rs, the fallback it hands `Names::unique`), Collada
// `node{i}` (dae.rs L5129), USD `node_{i}` (usd_write.rs, its `node_{n}`
// fallbacks), and glTF's own `nodes[i]` to assimp (the glb.rs and dae.rs line
// numbers as of 2026-11-08). A file already in that order keeps every index
// as its rank, which is all the index pairing of Ruling R25 held for.
// Khronos's Duck is not: its root's children are [2, 1], so the camera on
// node 1 in the file is the third node of the walk and litegen writes it as
// node 2. So the original's unnamed camera is `node 2`, and so is litegen's
// reading of it.
//
// Both the Node side (camlight-oracle.mjs, over a glTF JSON) and the page
// (render.js, over GLTFLoader's `parser.json`) import this one function.

// The depth-first rank of each node of `json`'s default scene (`scene`, else
// the first): pre-order, the scene's roots in file order, each node's children
// in file order. `ranks[i]` is node i's, or undefined for a node the scene
// does not reach. A node reached a second time keeps its first rank (litegen's
// reader refuses such a file; this walk simply ends there), and an index with
// no node is skipped.
export function depthFirstRanks(json) {
  const nodes = json?.nodes || [];
  const ranks = new Array(nodes.length).fill(undefined);
  const scene = (json?.scenes || [])[json?.scene ?? 0];
  const stack = [...(scene?.nodes || [])].reverse();
  let next = 0;
  while (stack.length) {
    const i = stack.pop();
    if (!nodes[i] || ranks[i] !== undefined) continue;
    ranks[i] = next++;
    const kids = nodes[i].children || [];
    for (let k = kids.length - 1; k >= 0; k--) stack.push(kids[k]);
  }
  return ranks;
}

// The key an unnamed node goes by, `node <rank>`; a node the default scene
// does not reach (no rank, and no partner in litegen's reading either) keeps
// its index.
export const unnamedKey = (ranks, i) => `node ${ranks?.[i] ?? i}`;
