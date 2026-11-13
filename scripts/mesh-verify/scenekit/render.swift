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
//             "mode": "albedo" | "silhouette" | "uv" | "wire", "time": <seconds or null>,
//             "clip": <Collada animation_clip id or null>,
//             "channel": <the uv set the uv mode draws, or null> }
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
// wire: the point and line elements alone (a Collada `<lines>`, a USD
// BasisCurves, a vertex-only PLY), white on transparent, for the harness's
// points-and-lines judge (verify.mjs, WIRE). Every other mode draws the
// triangle elements alone.
// uv: page/render.js's uv gradient, unlit and unfiltered, through the uv set
// `channel` (SCNMaterialProperty.mappingChannel: a geometry's texture
// coordinate sources are its mapping channels, in order), so each pixel's
// colour is its coordinate in that set. For the sets three.js's Collada and
// USD loaders do not build.
// lit: the file's own materials, as the importer built them, under an ambient
// light of `lightIntensity` lumens and a directional light of twice that, with
// the file's own lights removed; `only` keeps the meshes on the nodes it names
// (or below them) and hides the rest. Two such pictures at two intensities
// show whether a surface answers to the light (mesh-gaps item 6, unlit USD).
// lights: every surface white Lambert, lit by the file's own light on the
// node named one of `lightNodes` alone (every other light removed), its
// intensity scaled until the brightest pixel is near 230 (page/render.js's
// LIGHT_TARGET): the cross-renderer lights judge compares where a light
// falls and its colour, not how bright SceneKit draws it (page/render.js,
// compareLightPicture).
//
// `view: "file"` draws through the file's own first camera (the camera on the
// node named one of `cameraNodes`, or the first camera node in hierarchy
// order; a copy of it on a child of its node, see below) instead of
// `camera`, at `width` x `height`: the camera mode's judge, which a viewport
// of the camera's own aspect makes fair, as SceneKit takes a camera's aspect
// from its viewport. The last line printed also carries what SceneKit read
// of the file's lights and of the camera it drew through.

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
    /// The view's up direction, +Y when absent: a Z-up USD original is drawn
    /// in its own numbers, so its camera comes in with +Z up (verify.mjs).
    let up: [Double]?
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
    /// The uv set the uv mode draws through: its mapping channel.
    let channel: Int?
    /// The picture's size, where it is not `size` x `size`.
    let width: Int?
    let height: Int?
    /// "file": draw through the file's own first camera, or through the camera
    /// on the node named one of `cameraNodes`: SceneKit's USD importer lists a
    /// prim's children alphabetically (measured: staged's `ortho_camera_node`
    /// comes before its first camera, `persp_camera_node`, and `cube_node`
    /// before `ground_node`), so the file's first camera is named. A list,
    /// as a node with no name is known by its depth-first rank under each
    /// reader's name for it (verify.mjs, nameCandidates).
    let view: String?
    let cameraNodes: [String]?
    /// The lights mode's one light: the names of the node it hangs on.
    let lightNodes: [String]?
    /// The `lit` mode's light: an ambient light of this many lumens and a
    /// directional light of twice that, over the file's own materials.
    let lightIntensity: Double?
    /// The `lit` mode's group: only the meshes on a node named one of these, or
    /// below one, are drawn (the unlit boxes alone, or their lit control).
    let only: [String]?
    /// A USD file with several clips: SceneKit's USD importer keys one player
    /// per SkelAnimation prim by its path (`/clips/leg/walk`), plays all of them
    /// at once and reads no variant selection (measured, mesh-gaps item 6), so
    /// only the player of the prim named this is kept; the rigid `transform`
    /// player stays.
    let playClip: String?
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

/// The uv mode's gradient, texel for texel page/render.js's `uvTexture()`:
/// R = x, G = y, B an 8 x 8 checker, row 0 at the top (where a correctly
/// drawn image's top row is, V down, as in three.js's picture). Tagged linear,
/// as three.js's is `NoColorSpace`: both sample the bytes as they stand and
/// encode the result to sRGB on output.
func uvGradient() -> CGImage {
    let n = 256
    var px = [UInt8](repeating: 255, count: n * n * 4)
    for y in 0..<n {
        for x in 0..<n {
            let i = (y * n + x) * 4
            px[i] = UInt8(x)
            px[i + 1] = UInt8(y)
            px[i + 2] = ((x >> 5) + (y >> 5)) % 2 == 1 ? 200 : 40
        }
    }
    guard let space = CGColorSpace(name: CGColorSpace.linearSRGB),
          let provider = CGDataProvider(data: Data(px) as CFData),
          let image = CGImage(width: n, height: n, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: n * 4,
                              space: space, bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
                              provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
    else { fail("could not build the uv gradient") }
    return image
}

/// The same geometry without its colour source: the uv mode draws
/// coordinates, which a vertex colour would tint.
func withoutVertexColors(_ g: SCNGeometry) -> SCNGeometry {
    guard g.sources.contains(where: { $0.semantic == .color }) else { return g }
    let ng = SCNGeometry(sources: g.sources.filter { $0.semantic != .color }, elements: g.elements)
    ng.materials = g.materials
    return ng
}

/// A point element's indices, as numbers.
func pointIndices(_ e: SCNGeometryElement) -> [Int] {
    let size = e.bytesPerIndex
    guard size == 1 || size == 2 || size == 4 else { return [] }
    return e.data.withUnsafeBytes { (p: UnsafeRawBufferPointer) -> [Int] in
        (0..<(e.data.count / size)).map { i in
            switch size {
            case 1: return Int(p.load(fromByteOffset: i, as: UInt8.self))
            case 2: return Int(p.loadUnaligned(fromByteOffset: i * 2, as: UInt16.self))
            default: return Int(p.loadUnaligned(fromByteOffset: i * 4, as: UInt32.self))
            }
        }
    }
}

/// The same geometry with its triangle elements alone, or (`wire`) with its
/// point and line elements alone: nil where none is left. Each kept element
/// keeps its material (an element's material is the geometry's `index % count`).
/// SceneKit draws a Collada `<lines>` and a USD BasisCurves as line elements and
/// a vertex-only PLY as a point element, at its own size where the expected
/// picture's points are 4 px and its lines 1 px, so the modes that compare
/// pixels draw the triangles alone and the `wire` mode judges the rest, on the
/// box and centroid of what it covers (verify.mjs, WIRE).
func keepingElements(_ g: SCNGeometry, wire: Bool) -> SCNGeometry? {
    var kept: [SCNGeometryElement] = []
    var materials: [SCNMaterial] = []
    var replaced = false
    for (i, element) in g.elements.enumerated() {
        var e = element
        let isWire = e.primitiveType == .point || e.primitiveType == .line
        if isWire != wire { continue }
        if e.primitiveType == .point {
            // Model I/O gives a vertex-only PLY a point element of three indices
            // per vertex, 0 ..< 3n for n vertices (measured 2026-11-08 on macOS
            // 26.6.2: n = 1, 2, 4, 9, 50, 500 and 4,223 read 3, 6, 12, 27, 150, 1,500
            // and 12,669 indices). The 2n past the vertex source read beyond it
            // and SceneKit draws them at the origin: a point the file does not
            // hold. Only the indices that name a vertex are the file's points.
            if let n = g.sources(for: .vertex).first?.vectorCount {
                let indices = pointIndices(e)
                let real = indices.filter { $0 < n }
                if real.count != indices.count {
                    e = SCNGeometryElement(indices: real.map { UInt32($0) }, primitiveType: .point)
                    replaced = true
                }
            }
            // 4 px, as the expected picture's points: SceneKit's own size is 1.
            e.pointSize = 4
            e.minimumPointScreenSpaceRadius = 2
            e.maximumPointScreenSpaceRadius = 2
        }
        kept.append(e)
        if !g.materials.isEmpty { materials.append(g.materials[i % g.materials.count]) }
    }
    if kept.count == g.elements.count && !replaced { return g }
    if kept.isEmpty { return nil }
    let ng = SCNGeometry(sources: g.sources, elements: kept)
    ng.materials = materials
    return ng
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
        // A Collada node with an id and no name (litegen writes a node with no
        // name so, its id `node{i}`) comes in with no name; the scene source
        // still knows it by its id and hands back the same SCNNode (measured
        // 2026-11-06 on `staged` written with its names stripped), so it is
        // named by its id, as assimp names it.
        for id in source.identifiersOfEntries(withClass: SCNNode.self) {
            if let node = source.entryWithIdentifier(id, withClass: SCNNode.self), node.name == nil {
                node.name = id
            }
        }
    } else {
        scene = try SCNScene(url: modelURL, options: [.checkConsistency: true, .convertToYUp: false])
    }
} catch {
    fail("SceneKit could not load \(modelURL.lastPathComponent): \(error)")
}

var geometryCount = 0
var animationCount = 0
var texcoordSets = 0
let gradient: CGImage? = job.mode == "uv" ? uvGradient() : nil
// A USD file with several clips (`playClip`): SceneKit keys a USD xformOp
// animation's samples over the whole stage's time range (the longest clip's
// span), not over the span they were authored in, so the first clip's rigid
// motion runs slow by stage span / clip span (measured 2026-11-10, macOS 26.6.2,
// `clips` as a USDA with three clips: the prop's row at scene time 2 s equals
// usdrecord's at 1 s, and its `transform` key lasts 2.0 s against the walk
// player's 1.0 s). The rigid keys play at that ratio, so the selected clip's
// pictures are judged on where the prop is, not on how long SceneKit takes to
// get it there. litegen's writer gives the rigid samples the first clip's span.
var clipSpan = 0.0
var rigidSpan = 0.0
if let keep = job.playClip {
    scene.rootNode.enumerateHierarchy { node, _ in
        for key in node.animationKeys {
            guard let player = node.animationPlayer(forKey: key) else { continue }
            let span = Double(player.animation.duration)
            if key.hasPrefix("/") {
                if key.split(separator: "/").last.map(String.init) == keep { clipSpan = max(clipSpan, span) }
            } else {
                rigidSpan = max(rigidSpan, span)
            }
        }
    }
}
let rigidSpeed = (clipSpan > 0 && rigidSpan > clipSpan) ? rigidSpan / clipSpan : 1.0
scene.rootNode.enumerateHierarchy { node, _ in
    // Triangles alone, or in the `wire` mode points and lines alone. A skinned
    // node keeps its geometry whole (a skinner is built over it).
    if let g = node.geometry {
        let kept = keepingElements(g, wire: job.mode == "wire")
        if kept !== g {
            if let ng = kept {
                node.geometry = ng
                node.skinner?.baseGeometry = ng
            } else if node.skinner == nil {
                node.geometry = nil
            }
        }
    }
    if job.vertexColorsAreSRGB == true, let g = node.geometry {
        node.geometry = decodeVertexColors(g)
    }
    if job.mode == "uv", let g = node.geometry {
        let ng = withoutVertexColors(g)
        if ng !== g {
            node.geometry = ng
            node.skinner?.baseGeometry = ng
        }
    }
    // The `lit` mode's group: a mesh counts when it, or a node above it, is
    // named one of `only`.
    if job.mode == "lit", let only = job.only, node.geometry != nil {
        var named = false
        var up: SCNNode? = node
        while let n = up {
            if let name = n.name, only.contains(name) { named = true }
            up = n.parent
        }
        if !named { node.isHidden = true }
    }
    if let g = node.geometry {
        geometryCount += 1
        texcoordSets = max(texcoordSets, g.sources(for: .texcoord).count)
        for m in g.materials {
            switch job.mode {
            case "silhouette", "wire":
                m.lightingModel = .constant
                m.diffuse.contents = NSColor.white
                m.multiply.contents = NSColor.white
                m.emission.contents = NSColor.black
            case "uv":
                // The gradient through the chosen set, unlit and unfiltered,
                // as page/render.js's uv modes draw it: repeat-wrapped and
                // nearest-sampled, half a texel in (UV_TEXTURE.offset).
                m.lightingModel = .constant
                m.diffuse.contents = gradient
                m.diffuse.mappingChannel = job.channel ?? 0
                m.diffuse.wrapS = .repeat
                m.diffuse.wrapT = .repeat
                m.diffuse.minificationFilter = .nearest
                m.diffuse.magnificationFilter = .nearest
                m.diffuse.mipFilter = .none
                m.diffuse.contentsTransform = SCNMatrix4MakeTranslation(0.5 / 256, 0.5 / 256, 0)
                m.multiply.contents = NSColor.white
                m.emission.contents = NSColor.black
                m.ambientOcclusion.contents = nil
            case "lit":
                // The importer's own materials, untouched.
                break
            case "lights":
                // White Lambert under the one light kept below.
                m.lightingModel = .lambert
                m.diffuse.contents = NSColor.white
                m.multiply.contents = NSColor.white
                m.emission.contents = NSColor.black
                m.ambient.contents = NSColor.black
                m.specular.contents = NSColor.black
                m.ambientOcclusion.contents = nil
                m.selfIllumination.contents = nil
                m.normal.contents = nil
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
        if let keep = job.playClip, key.hasPrefix("/"), key.split(separator: "/").last.map(String.init) != keep {
            node.removeAnimation(forKey: key)
            continue
        }
        if let player = node.animationPlayer(forKey: key) {
            animationCount += 1
            player.animation.usesSceneTimeBase = true
            player.animation.isRemovedOnCompletion = false
            player.animation.fillsForward = true
            player.animation.fillsBackward = true
            if job.playClip != nil, !key.hasPrefix("/") { player.speed = CGFloat(rigidSpeed) }
            player.play()
        }
    }
    // A Collada weight animation hangs on the node's SCNMorpher, not on the
    // node (item 8, Task 0: `node.animationKeys` is empty for it), so it is
    // driven from scene time and, below, removed for the rest pose, like any
    // other. Without this the rest picture shows the animation's first key
    // instead of the default weights, and a time with no clip shows that key
    // throughout.
    if let morpher = node.morpher {
        for key in morpher.animationKeys {
            if let player = morpher.animationPlayer(forKey: key) {
                animationCount += 1
                player.animation.usesSceneTimeBase = true
                player.animation.isRemovedOnCompletion = false
                player.animation.fillsForward = true
                player.animation.fillsBackward = true
                player.play()
            }
        }
    }
}

if job.time == nil || job.clip != nil {
    scene.rootNode.enumerateHierarchy { node, _ in
        node.removeAllAnimations()
        node.morpher?.removeAllAnimations()
    }
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

// What SceneKit read of the file's lights, before any is removed; in the
// lights mode, every light but the one on the node named one of `lightNodes`
// goes.
func components(_ c: Any) -> [Double] {
    guard let col = c as? NSColor, let s = col.usingColorSpace(.extendedSRGB) else { return [] }
    return [Double(s.redComponent), Double(s.greenComponent), Double(s.blueComponent)]
}
var lightsRead: [[String: Any]] = []
var keptLight: SCNLight? = nil
scene.rootNode.enumerateHierarchy { node, _ in
    guard let l = node.light else { return }
    lightsRead.append([
        "node": node.name ?? "", "type": l.type.rawValue, "color": components(l.color), "intensity": Double(l.intensity),
        "spotInnerAngle": Double(l.spotInnerAngle), "spotOuterAngle": Double(l.spotOuterAngle),
        "attenuationStartDistance": Double(l.attenuationStartDistance), "attenuationEndDistance": Double(l.attenuationEndDistance),
        "attenuationFalloffExponent": Double(l.attenuationFalloffExponent),
    ])
    if job.mode == "lit" {
        node.light = nil
    } else if job.mode == "lights" {
        if let names = job.lightNodes, names.contains(node.name ?? ""), keptLight == nil {
            keptLight = l
        } else {
            node.light = nil
        }
    }
}
if job.mode == "lights" && keptLight == nil { fail("SceneKit read no light on a node named any of \(job.lightNodes ?? [])") }

// The `lit` mode's light: an ambient one and a directional one from the upper
// left front, at the intensity the job names.
if job.mode == "lit" {
    let level = CGFloat(job.lightIntensity ?? 1000)
    let ambient = SCNNode()
    ambient.light = SCNLight()
    ambient.light?.type = .ambient
    ambient.light?.color = NSColor.white
    ambient.light?.intensity = level
    scene.rootNode.addChildNode(ambient)
    let sun = SCNNode()
    sun.light = SCNLight()
    sun.light?.type = .directional
    sun.light?.color = NSColor.white
    sun.light?.intensity = level * 2
    sun.simdEulerAngles = SIMD3<Float>(-0.6, -0.5, 0)
    scene.rootNode.addChildNode(sun)
}

// The point of view: the file's own first camera, or the job's.
var cameraRead: [String: Any]? = nil
let pointOfView: SCNNode
if job.view == "file" {
    var first: SCNNode? = nil
    scene.rootNode.enumerateHierarchy { node, stop in
        if node.camera != nil && (job.cameraNodes == nil || job.cameraNodes!.contains(node.name ?? "")) {
            first = node
            stop.pointee = true
        }
    }
    guard let node = first, let c = node.camera else {
        fail("SceneKit read no camera\(job.cameraNodes.map { " on a node named any of \($0)" } ?? "") in \(modelURL.lastPathComponent)")
    }
    cameraRead = [
        "node": node.name ?? "", "fieldOfView": Double(c.fieldOfView), "projectionDirection": c.projectionDirection == .vertical ? "vertical" : "horizontal",
        "zNear": c.zNear, "zFar": c.zFar, "orthographic": c.usesOrthographicProjection, "orthographicScale": c.orthographicScale,
    ]
    // Drawn through a copy of the camera on a child of its node, which keeps
    // its frame and its animation: through an imported USD camera node itself
    // SceneKit draws almost nothing (measured on `staged`: 9 pixels of
    // 44,154), while a copy of the same camera on the same frame draws them
    // all, as the Collada camera node does.
    let child = SCNNode()
    child.camera = c.copy() as? SCNCamera
    node.addChildNode(child)
    pointOfView = child
} else {
    let camNode = SCNNode()
    let cam = SCNCamera()
    cam.fieldOfView = CGFloat(job.camera.fov)
    cam.projectionDirection = .vertical
    cam.zNear = job.camera.near
    cam.zFar = job.camera.far
    camNode.camera = cam
    camNode.simdPosition = SIMD3<Float>(job.camera.position.map { Float($0) })
    camNode.simdLook(at: SIMD3<Float>(job.camera.target.map { Float($0) }), up: SIMD3<Float>((job.camera.up ?? [0, 1, 0]).map { Float($0) }), localFront: SIMD3<Float>(0, 0, -1))
    scene.rootNode.addChildNode(camNode)
    pointOfView = camNode
}
scene.background.contents = NSColor.clear

guard let device = MTLCreateSystemDefaultDevice() else { fail("no Metal device") }
let renderer = SCNRenderer(device: device, options: nil)
renderer.scene = scene
renderer.pointOfView = pointOfView
renderer.autoenablesDefaultLighting = false
let width = CGFloat(job.width ?? job.size ?? 256)
let height = CGFloat(job.height ?? job.size ?? 256)
let t = job.time ?? 0
renderer.sceneTime = t
func shoot() -> NSImage { renderer.snapshot(atTime: t, with: CGSize(width: width, height: height), antialiasingMode: .none) }
var image = shoot()

// The lights mode's exposure: the kept light's intensity scaled until the
// brightest covered pixel is near 230 of 255.
func brightest(_ image: NSImage) -> Int {
    guard let tiff = image.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff) else { return 0 }
    var m: CGFloat = 0
    for y in 0..<rep.pixelsHigh {
        for x in 0..<rep.pixelsWide {
            guard let c = rep.colorAt(x: x, y: y), c.alphaComponent > 0 else { continue }
            m = max(m, c.redComponent, c.greenComponent, c.blueComponent)
        }
    }
    return Int((m * 255).rounded())
}
var exposure = 1.0
if let l = keptLight {
    let base = l.intensity
    // A light too faint to show at its own intensity is brightened 16× at a
    // time until a pixel shows, at most 16^10, as page/render.js's exposure
    // search does; one that still shows nothing lights nothing.
    var tries = 0
    while brightest(image) == 0 && tries < 10 {
        exposure *= 16
        l.intensity = base * CGFloat(exposure)
        image = shoot()
        tries += 1
    }
    if brightest(image) == 0 {
        exposure = 1
        l.intensity = base
        image = shoot()
    }
    for _ in 0..<16 {
        let m = brightest(image)
        if m == 0 { break }
        if m >= 250 {
            exposure /= 4
        } else {
            exposure *= Double(srgbToLinear(230.0 / 255) / srgbToLinear(Float(m) / 255))
        }
        l.intensity = base * CGFloat(exposure)
        image = shoot()
        if m < 250 { break }
    }
}

guard let tiff = image.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
      let png = rep.representation(using: .png, properties: [:])
else { fail("could not encode PNG") }
do {
    try png.write(to: outURL)
} catch {
    fail("could not write \(outURL.path): \(error)")
}
var stats: [String: Any] = ["geometries": geometryCount, "animations": animationCount, "texcoords": texcoordSets, "lights": lightsRead]
if let c = cameraRead { stats["camera"] = c }
if keptLight != nil { stats["exposure"] = exposure }
guard let line = try? JSONSerialization.data(withJSONObject: stats), let text = String(data: line, encoding: .utf8) else { fail("could not encode the stats") }
print(text)
