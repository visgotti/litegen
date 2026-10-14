// A second, independent renderer for the mesh verification harness: Apple's
// SceneKit, which loads USD/USDZ, Collada, OBJ (+MTL), PLY and STL through
// Model I/O — the same stack macOS Quick Look uses.
//
// three.js is the primary oracle (it is the engine behind the dashboard), but
// its USD and Collada loaders have documented gaps (USDA skeletal rotations,
// blend shapes, Collada vertex colours). Where they are blind, this one looks.
//
//   render <model> <out.png> <job.json>
//
// job.json: { "camera": {position, target, fov, near, far}, "size": 256,
//             "mode": "albedo" | "silhouette", "time": <seconds or null>,
//             "clip": <Collada animation_clip id or null> }
//
// time null is the rest pose: every animation removed, as three.js shows a
// file before any clip plays. A clip id plays that clip alone: SceneKit
// otherwise plays every animation in the file at once, where the comparison
// (and three.js) plays one.
//
// albedo: every material switched to SceneKit's constant (unlit) lighting
// model, so the picture is base colour x texture x vertex colour — comparable
// with three.js's unlit albedo mode up to filtering and rounding.
// silhouette: white on transparent, comparing coverage only (geometry,
// transforms, skinning, morphs, animation), independent of materials.

import AppKit
import Foundation
import Metal
import SceneKit

struct Camera: Decodable {
    let position: [Double]
    let target: [Double]
    let fov: Double
    let near: Double
    let far: Double
}

struct Job: Decodable {
    let camera: Camera
    let size: Int?
    let mode: String
    let time: Double?
    /// Model I/O loads OBJ (`v x y z r g b`) and PLY vertex colours as linear;
    /// three.js, MeshLab and Blender — and litegen's writers — treat them as
    /// sRGB. When set, loaded vertex colours are decoded from sRGB so the
    /// comparison tests the data rather than the two conventions.
    let vertexColorsAreSRGB: Bool?
    let clip: String?
}

func srgbToLinear(_ c: Float) -> Float {
    if c <= 0 { return 0 }
    if c >= 1 { return 1 }
    return c <= 0.04045 ? c / 12.92 : powf((c + 0.055) / 1.055, 2.4)
}

/// The same geometry with its colour source re-decoded from sRGB.
func decodeVertexColors(_ g: SCNGeometry) -> SCNGeometry {
    var sources = g.sources
    guard let i = sources.firstIndex(where: { $0.semantic == .color }) else { return g }
    let src = sources[i]
    let n = src.vectorCount
    let comps = src.componentsPerVector
    let bpc = src.bytesPerComponent
    var out = [Float]()
    out.reserveCapacity(n * comps)
    src.data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
        for v in 0..<n {
            for c in 0..<comps {
                let o = src.dataOffset + v * src.dataStride + c * bpc
                var x: Float
                if src.usesFloatComponents && bpc == 4 {
                    x = raw.loadUnaligned(fromByteOffset: o, as: Float.self)
                } else if src.usesFloatComponents && bpc == 8 {
                    x = Float(raw.loadUnaligned(fromByteOffset: o, as: Double.self))
                } else if bpc == 1 {
                    x = Float(raw[o]) / 255
                } else if bpc == 2 {
                    x = Float(raw.loadUnaligned(fromByteOffset: o, as: UInt16.self)) / 65535
                } else {
                    x = 1
                }
                out.append(c < 3 ? srgbToLinear(x) : x)
            }
        }
    }
    let data = out.withUnsafeBufferPointer { Data(buffer: $0) }
    sources[i] = SCNGeometrySource(data: data, semantic: .color, vectorCount: n, usesFloatComponents: true,
                                   componentsPerVector: comps, bytesPerComponent: 4, dataOffset: 0,
                                   dataStride: comps * 4)
    let ng = SCNGeometry(sources: sources, elements: g.elements)
    ng.materials = g.materials
    return ng
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}

let args = CommandLine.arguments
guard args.count == 4 else { fail("usage: render <model> <out.png> <job.json>") }
let modelURL = URL(fileURLWithPath: args[1])
let outURL = URL(fileURLWithPath: args[2])
guard let jobData = try? Data(contentsOf: URL(fileURLWithPath: args[3])),
      let job = try? JSONDecoder().decode(Job.self, from: jobData)
else { fail("bad job json") }

let scene: SCNScene
var colladaSource: SCNSceneSource? = nil
do {
    if modelURL.pathExtension.lowercased() == "dae" {
        // SceneKit imports a Collada FILE in an XPC service, and under the
        // sandbox this harness runs in that fails for every Collada file —
        // the research's hand-checked reference included — with
        // NSCocoaErrorDomain 4864 "Failed to retrieve scene from XPC
        // service". Imported from data, SceneKit's same Collada importer
        // loads it in-process.
        // SceneKit resolves relative image paths against these URLs with the
        // last path component removed, so the model's own URL — not its
        // directory, which would lose a level — is the one that works.
        let data = try Data(contentsOf: modelURL)
        guard let source = SCNSceneSource(data: data, options: [.assetDirectoryURLs: [modelURL]]) else {
            fail("SceneKit could not open \(modelURL.lastPathComponent) as a scene source")
        }
        scene = try source.scene(options: [.checkConsistency: true, .convertToYUp: false, .assetDirectoryURLs: [modelURL]])
        colladaSource = source
    } else {
        scene = try SCNScene(url: modelURL, options: [.checkConsistency: true, .convertToYUp: false])
    }
} catch {
    fail("SceneKit could not load \(modelURL.lastPathComponent): \(error)")
}

var geometryCount = 0
var animationCount = 0
scene.rootNode.enumerateHierarchy { node, _ in
    if job.vertexColorsAreSRGB == true, let g = node.geometry {
        node.geometry = decodeVertexColors(g)
    }
    if let g = node.geometry {
        geometryCount += 1
        for m in g.materials {
            switch job.mode {
            case "silhouette":
                m.lightingModel = .constant
                m.diffuse.contents = NSColor.white
                m.multiply.contents = NSColor.white
                m.emission.contents = NSColor.black
            default:
                // Unlit: diffuse contents (colour or texture) x vertex colour.
                m.lightingModel = .constant
                m.emission.contents = NSColor.black
                // Trilinear, as three.js samples by default: SceneKit's
                // default is no mipmapping, and the two then disagree along
                // every texture edge for reasons that are not in the data.
                m.diffuse.minificationFilter = .linear
                m.diffuse.magnificationFilter = .linear
                m.diffuse.mipFilter = .linear
            }
            m.isDoubleSided = false
            m.cullMode = .back
            m.transparencyMode = .aOne
        }
    }
    // Drive every animation from scene time, so a snapshot at time t shows
    // the pose at t rather than wherever wall-clock playback has got to.
    for key in node.animationKeys {
        if let player = node.animationPlayer(forKey: key) {
            animationCount += 1
            player.animation.usesSceneTimeBase = true
            player.animation.isRemovedOnCompletion = false
            player.animation.fillsForward = true
            player.animation.fillsBackward = true
            player.play()
        }
    }
}

if job.time == nil || job.clip != nil {
    scene.rootNode.enumerateHierarchy { node, _ in node.removeAllAnimations() }
    animationCount = 0
}
if job.time != nil, let clip = job.clip {
    // A Collada clip is an entry of the scene source: a group of the
    // animations it instances, key paths naming their nodes from the root.
    guard let source = colladaSource, let ca = source.entryWithIdentifier(clip, withClass: CAAnimation.self) else {
        fail("no animation clip \(clip) in \(modelURL.lastPathComponent)")
    }
    let anim = SCNAnimation(caAnimation: ca)
    anim.usesSceneTimeBase = true
    anim.isRemovedOnCompletion = false
    anim.fillsForward = true
    anim.fillsBackward = true
    let player = SCNAnimationPlayer(animation: anim)
    scene.rootNode.addAnimationPlayer(player, forKey: "clip")
    player.play()
    animationCount = 1
}

let camNode = SCNNode()
let cam = SCNCamera()
cam.fieldOfView = CGFloat(job.camera.fov)
cam.projectionDirection = .vertical
cam.zNear = job.camera.near
cam.zFar = job.camera.far
camNode.camera = cam
camNode.simdPosition = SIMD3<Float>(job.camera.position.map { Float($0) })
camNode.simdLook(at: SIMD3<Float>(job.camera.target.map { Float($0) }), up: SIMD3<Float>(0, 1, 0), localFront: SIMD3<Float>(0, 0, -1))
scene.rootNode.addChildNode(camNode)
scene.background.contents = NSColor.clear

guard let device = MTLCreateSystemDefaultDevice() else { fail("no Metal device") }
let renderer = SCNRenderer(device: device, options: nil)
renderer.scene = scene
renderer.pointOfView = camNode
renderer.autoenablesDefaultLighting = false
let size = CGFloat(job.size ?? 256)
let t = job.time ?? 0
renderer.sceneTime = t
let image = renderer.snapshot(atTime: t, with: CGSize(width: size, height: size), antialiasingMode: .none)

guard let tiff = image.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
      let png = rep.representation(using: .png, properties: [:])
else { fail("could not encode PNG") }
do {
    try png.write(to: outURL)
} catch {
    fail("could not write \(outURL.path): \(error)")
}
print("{\"geometries\": \(geometryCount), \"animations\": \(animationCount)}")
