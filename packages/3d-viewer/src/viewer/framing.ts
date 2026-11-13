// Where the camera goes for a model: the dashboard viewer's framing
// (dashboard/src/components/ModelViewer.tsx:95-97, model-viewer's defaults:
// orbit "0deg 75deg 105%") applied to the model's bounds as
// scripts/mesh-verify/page/render.js `frameCamera` measures them (558-573).
import * as THREE from 'three';

/** Vertical field of view, degrees. */
export const FOV = 30;
/** model-viewer's orbit: from the front (theta 0), 75° down from straight
 *  above (phi), at 105% of the distance that fits the model. */
const THETA = 0;
const PHI = THREE.MathUtils.degToRad(75);
const RADIUS_SCALE = 1.05;

export interface Framing {
  position: THREE.Vector3;
  target: THREE.Vector3;
  near: number;
  far: number;
  /** The model's size along each axis, in its own units. */
  size: THREE.Vector3;
}

export function frame(root: THREE.Object3D, aspect: number): Framing {
  root.updateMatrixWorld(true);
  const box = new THREE.Box3().setFromObject(root, true);
  if (box.isEmpty()) box.set(new THREE.Vector3(-0.5, -0.5, -0.5), new THREE.Vector3(0.5, 0.5, 0.5));
  const target = box.getCenter(new THREE.Vector3());
  const size = box.getSize(new THREE.Vector3());
  const radius = Math.max(size.length() / 2, 1e-6);
  const vfov = THREE.MathUtils.degToRad(FOV);
  const hfov = 2 * Math.atan(Math.tan(vfov / 2) * aspect);
  const distance = (radius / Math.sin(Math.min(vfov, hfov) / 2)) * RADIUS_SCALE;
  const direction = new THREE.Vector3(Math.sin(PHI) * Math.sin(THETA), Math.cos(PHI), Math.sin(PHI) * Math.cos(THETA));
  return {
    position: target.clone().addScaledVector(direction, distance),
    target,
    near: Math.max(distance - radius * 2, distance / 1000),
    far: distance + radius * 2,
    size,
  };
}
