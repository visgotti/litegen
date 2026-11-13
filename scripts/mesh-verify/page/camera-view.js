// The size of a camera picture, and the check that a picture rendered outside
// the page (usdrecord) has the size its expected picture was drawn at. Pure
// functions, imported by page/render.js and by camlight-oracle.test.mjs.

// A camera with no aspect ratio of its own is written by litegen to FBX and
// USD at 16:9 (usd_write.rs `write_camera`, its `aspect.unwrap_or(fbx::UNSET_ASPECT)`;
// fbx.rs `UNSET_ASPECT`; 3d-format-conversion.md §5: "For other readers an unset aspect ratio
// is written as 16:9", reported as a `cameras` loss), so usdrecord, which draws
// a camera at its own aspect, draws it 16:9 (followups 2026-11-08, C). The
// expected picture it is compared with is drawn at that aspect too.
export const UNSET_ASPECT = 16 / 9;

// A camera picture's size: SIZE high, SIZE x aspect wide (at most 4 x SIZE);
// a square for an aspect that is not a positive number.
export function cameraViewFor(aspect, size) {
  const a = Number.isFinite(aspect) && aspect > 0 ? aspect : 1;
  return { width: Math.min(4 * size, Math.max(1, Math.round(size * a))), height: size };
}

// Whether a picture of `natural` size ({ width, height }) is the size its
// expected picture `want` was drawn at: {} when it is; { rescaled } when it is
// one row short, said; else { error }, never scaled.
//
// usdrecord --camera draws at the camera's own aspect. Its height is the width
// over the aspect, truncated (pxr/usdImaging/usdAppUtils/frameRecorder.cpp
// L386-L391 at v25.05), so an f32 aspect a hair above the true one loses a
// row: litegen's 0.36 / 0.24 apertures make 384 / 1.5000001, 255 rows. That
// one row is scaled away (half a pixel at the far edge, inside the
// silhouette's one-pixel tolerance) and said; any other size is an error, so a
// camera whose aspect really changed still fails.
export function externalPictureSize(natural, want) {
  const { width: W, height: H } = want;
  if (natural.width === W && natural.height === H) return {};
  if (natural.width !== W || natural.height !== H - 1) return { error: `the picture is ${natural.width}x${natural.height}, not ${W}x${H}` };
  return { rescaled: `drawn ${W}x${H - 1}, usdrecord's truncated height for this camera's aspect (frameRecorder.cpp L386-L391), scaled to ${W}x${H}` };
}
