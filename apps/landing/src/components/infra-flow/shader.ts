/**
 * WGSL for the infrastructure-flow visual — a radial "router web".
 *
 * A single fullscreen triangle drives a fragment shader that paints everything
 * *behind* the crisp HTML labels:
 *   - a slow violet flow-field background,
 *   - a soft light that tracks the cursor, plus a gravitational *lens* that
 *     bends the nearby light-paths around the pointer,
 *   - gently-bowed glowing spokes from the central gateway to every provider,
 *     carrying faint outbound "request" dots and bright inbound "media tiles"
 *     (the generated image/video flowing back), tinted by each provider's kind,
 *   - a slow rotating conic *scan beam* sweeping the hub that lifts whatever
 *     spoke it crosses — an always-on "the GPU is doing real work" heartbeat,
 *   - a soft pulsing gateway hub core,
 *   - soft rounded-box halos around every node that bloom when hovered (focus).
 *
 * Node data arrives via a uniform. The CPU side packs nodes in the order
 * [app, gateway, ...providers]; the shader hardcodes the edge topology from
 * that ordering (app→gateway, gateway→each provider).
 *
 * Uniform layout (std140-compatible; mirrored exactly by renderer.ts — the
 * array length here MUST equal MAX_NODES there):
 *   res   : vec4  = (canvas.x, canvas.y, dpr, nodeCount)
 *   ctl   : vec4  = (time seconds, reducedMotion 0|1, pad, pad)
 *   mouse : vec4  = (pointer.x uv, pointer.y uv, inside 0|1, pad)
 *   geom  : vec4[24] = per node (centerX uv, centerY uv, radius uv-x, focus 0..1)
 *   meta  : vec4[24] = per node (role 0app/1gate/2prov, kind 0img/1vid,
 *                                halfWidth/stageW, halfHeight/stageH)
 *   nodeFlags : vec4[24] = per node (.x input-vocab bitmask text1|image2|multi4,
 *                                    .y output-modality bitmask image1|video2, .zw pad)
 */
export const FLOW_WGSL = /* wgsl */ `
struct U {
  res   : vec4<f32>,
  ctl   : vec4<f32>,
  mouse : vec4<f32>,
  geom  : array<vec4<f32>, 24>,
  nodeMeta : array<vec4<f32>, 24>,   // 'meta' is a reserved word in WGSL
  nodeFlags : array<vec4<f32>, 24>,  // (.x=input bitmask text1|image2|multi4, .y=output bitmask image1|video2)
};
@group(0) @binding(0) var<uniform> u : U;

struct VSOut {
  @builtin(position) pos : vec4<f32>,
  @location(0) uv : vec2<f32>,   // 0..1, y-down to match the DOM
};

@vertex
fn vs(@builtin(vertex_index) vi : u32) -> VSOut {
  var p = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 3.0, -1.0),
    vec2<f32>(-1.0,  3.0),
  );
  var out : VSOut;
  let xy = p[vi];
  out.pos = vec4<f32>(xy, 0.0, 1.0);
  out.uv = vec2<f32>(xy.x * 0.5 + 0.5, 1.0 - (xy.y * 0.5 + 0.5));
  return out;
}

// ---- noise -----------------------------------------------------------------
fn hash21(p : vec2<f32>) -> f32 {
  let h = dot(p, vec2<f32>(127.1, 311.7));
  return fract(sin(h) * 43758.5453123);
}
fn noise(p : vec2<f32>) -> f32 {
  let i = floor(p);
  let f = fract(p);
  let a = hash21(i);
  let b = hash21(i + vec2<f32>(1.0, 0.0));
  let c = hash21(i + vec2<f32>(0.0, 1.0));
  let d = hash21(i + vec2<f32>(1.0, 1.0));
  let uu = f * f * (3.0 - 2.0 * f);
  return mix(mix(a, b, uu.x), mix(c, d, uu.x), uu.y);
}
fn fbm(p0 : vec2<f32>) -> f32 {
  var p = p0;
  var v = 0.0;
  var amp = 0.5;
  for (var i = 0; i < 5; i = i + 1) {
    v = v + amp * noise(p);
    p = p * 2.02;
    amp = amp * 0.5;
  }
  return v;
}

// distance from p to segment a-b, plus param along it (0..1)
fn segInfo(p : vec2<f32>, a : vec2<f32>, b : vec2<f32>) -> vec2<f32> {
  let pa = p - a;
  let ba = b - a;
  let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
  let d = length(pa - ba * h);
  return vec2<f32>(d, h);
}

// rounded-box signed distance (used for the media tiles + node halos)
fn sdRoundBox(p : vec2<f32>, b : vec2<f32>, r : f32) -> f32 {
  let q = abs(p) - b + vec2<f32>(r);
  return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

// quadratic Bézier point a→b with control c
fn bez(a : vec2<f32>, c : vec2<f32>, b : vec2<f32>, t : f32) -> vec2<f32> {
  let s = 1.0 - t;
  return s * s * a + 2.0 * s * t * c + t * t * b;
}

// Cheap 1D hash for per-packet identity (which glyph this emission shows).
fn hash1(n : f32) -> f32 {
  return fract(sin(n * 12.9898) * 43758.5453123);
}

// Choose one accepted input type for a packet. mask bits: text=1, image=2,
// multi=4. r in [0,1). Returns 0=text, 1=image, 2=multi. Falls back to text
// when the mask is empty (app/gateway trunk).
fn pickVocab(mask : i32, r : f32) -> i32 {
  var opts = array<i32, 3>(0, 0, 0);
  var n = 0;
  if ((mask & 1) != 0) { opts[n] = 0; n = n + 1; }
  if ((mask & 2) != 0) { opts[n] = 1; n = n + 1; }
  if ((mask & 4) != 0) { opts[n] = 2; n = n + 1; }
  if (n == 0) { return 0; }
  let idx = clamp(i32(floor(r * f32(n))), 0, n - 1);
  return opts[idx];
}

// Choose one output modality for a returning media tile. mask bits: image=1,
// video=2. r in [0,1). Returns 0=image, 1=video. A both-capable provider (mask
// 3) alternates, so the diagram shows that a vendor returns more than one kind.
fn pickOutput(mask : i32, r : f32) -> i32 {
  var opts = array<i32, 2>(0, 0);
  var n = 0;
  if ((mask & 1) != 0) { opts[n] = 0; n = n + 1; }
  if ((mask & 2) != 0) { opts[n] = 1; n = n + 1; }
  if (n == 0) { return 0; }
  let idx = clamp(i32(floor(r * f32(n))), 0, n - 1);
  return opts[idx];
}

// An outbound input "packet" glyph centered at q=0. kind: 0 text (tile with
// scribble strokes), 1 single image (framed square), 2 multi-image (offset
// squares). Cool/desaturated so the inbound media still dominates the route.
fn glyph(q : vec2<f32>, kind : i32, boost : f32) -> vec3<f32> {
  let cool = vec3<f32>(0.55, 0.62, 0.95);
  let amp = 0.45 + 0.55 * boost;
  if (kind == 0) {
    let sd = sdRoundBox(q, vec2<f32>(0.013, 0.010), 0.004);
    let frame = smoothstep(0.0022, 0.0, abs(sd)) * 0.6;
    var lines = 0.0;
    for (var k = 0; k < 2; k = k + 1) {
      let yy = -0.004 + f32(k) * 0.008;
      let wob = 0.0016 * sin(q.x * 420.0);
      lines = lines + smoothstep(0.0014, 0.0, abs(q.y - yy - wob)) * step(abs(q.x), 0.009);
    }
    return cool * (frame + lines * 0.5) * amp;
  }
  if (kind == 1) {
    let sd = sdRoundBox(q, vec2<f32>(0.011, 0.011), 0.003);
    let body = smoothstep(0.001, 0.0, sd) * 0.32;
    let frame = smoothstep(0.0022, 0.0, abs(sd)) * 0.72;
    return cool * (body + frame) * amp;
  }
  // kind == 2: two offset squares
  var acc = 0.0;
  for (var k = 0; k < 2; k = k + 1) {
    let off = vec2<f32>(-0.004 + f32(k) * 0.008, 0.004 - f32(k) * 0.008);
    let sd = sdRoundBox(q - off, vec2<f32>(0.0075, 0.0075), 0.0025);
    acc = acc + smoothstep(0.0019, 0.0, abs(sd)) * 0.7 + smoothstep(0.0009, 0.0, sd) * 0.22;
  }
  return cool * acc * amp;
}

// Distance to a quadratic Bézier, approximated with 3 line subsegments (no
// per-pixel exact-root solve — far cheaper and visually identical at this
// scale). Returns (minDistance, paramAlongCurve 0..1).
fn curveDist(p : vec2<f32>, a : vec2<f32>, c : vec2<f32>, b : vec2<f32>) -> vec2<f32> {
  var prev = a;
  var bestD = 1.0e9;
  var bestAlong = 0.0;
  for (var k = 0; k < 3; k = k + 1) {
    let t1 = f32(k + 1) / 3.0;
    let pt = bez(a, c, b, t1);
    let inf = segInfo(p, prev, pt);
    if (inf.x < bestD) {
      bestD = inf.x;
      bestAlong = (f32(k) + inf.y) / 3.0;
    }
    prev = pt;
  }
  return vec2<f32>(bestD, bestAlong);
}

// The glowing base line for a segment a->b. bowScale curves it (0 = straight).
fn trace(p : vec2<f32>, a : vec2<f32>, b : vec2<f32>, boost : f32, bowScale : f32) -> vec3<f32> {
  let dir = b - a;
  let len = length(dir);
  let perp = normalize(vec2<f32>(-dir.y, dir.x));
  let c = (a + b) * 0.5 + perp * (len * bowScale);
  let d = curveDist(p, a, c, b).x;
  let violet = vec3<f32>(0.46, 0.36, 1.0);
  var col = vec3<f32>(0.0);
  col = col + violet * smoothstep(0.0026, 0.0, d) * (0.26 + 0.55 * boost);
  col = col + violet * smoothstep(0.055, 0.0, d) * (0.045 + 0.34 * boost);
  return col;
}

// Position along the full two-leg route client(app) -> gateway -> model, t 0..1.
// The shared trunk leg (app->gate) is crossed quickly (first SPLIT of the
// timeline) so it doesn't crowd; the rest is the bowed gateway->model spoke.
fn routePos(app : vec2<f32>, gate : vec2<f32>, model : vec2<f32>, c : vec2<f32>, t : f32) -> vec2<f32> {
  let SPLIT = 0.30;
  if (t < SPLIT) {
    return mix(app, gate, t / SPLIT);
  }
  return bez(gate, c, model, (t - SPLIT) / (1.0 - SPLIT));
}

// Moving payloads for one spoke, routed end to end through the hub:
//   OUTBOUND request glyph: client(app) -> gateway -> model (the prompt reaching
//     the model — one of the provider's accepted input types).
//   INBOUND media tile: model -> gateway -> client (the result coming back to you).
// Both duty-cycled + staggered per spoke so the web stays calm; each draws only
// near its own moving centre (cheap per-pixel reject), so a packet on the shared
// trunk leg renders even though it's far from this spoke's base line.
fn packets(p : vec2<f32>, app : vec2<f32>, gate : vec2<f32>, model : vec2<f32>,
           mt : f32, boost : f32, rm : f32, vocab : i32, spokeId : f32, outMask : f32) -> vec3<f32> {
  var col = vec3<f32>(0.0);
  let dir = model - gate;
  let len = length(dir);
  let perp = normalize(vec2<f32>(-dir.y, dir.x));
  let c = (gate + model) * 0.5 + perp * (len * 0.12);   // control for the spoke leg
  let imgTint = vec3<f32>(1.0, 0.30, 0.85);   // hot magenta — image
  let vidTint = vec3<f32>(0.18, 0.80, 1.0);   // electric cyan — video

  // OUTBOUND request: client -> gateway -> model.
  // Speed is CONSTANT (not boost-scaled). Phase is mt*spd and mt grows unbounded,
  // so coupling spd to boost made any boost change — e.g. the scan beam sweeping
  // onto this spoke, or a hover — jump the packet by mt*delta and streak it across
  // the wire. boost still drives the glyph's brightness below, just not velocity.
  let ospd = 0.10;
  let oRaw = mt * ospd + hash1(spokeId * 3.17 + 1.0);
  let oFrac = select(fract(oRaw), 0.25, rm > 0.5);     // park mid-route under rm
  if (oFrac < 0.45) {
    let oCyc = select(floor(oRaw), 0.0, rm > 0.5);
    let ph = oFrac / 0.45;
    let kindSel = pickVocab(vocab, hash1(spokeId * 7.13 + oCyc * 3.7));
    let center = routePos(app, gate, model, c, ph);
    let qo = p - center;
    if (dot(qo, qo) < 0.0026) {
      let life = smoothstep(0.0, 0.10, ph) * smoothstep(0.0, 0.16, 1.0 - ph);
      col = col + glyph(qo, kindSel, boost) * life;
    }
  }

  // INBOUND result: the generated media travels model -> gateway -> client,
  // growing + gaining vibrancy across the whole trip so it reads as "resolving"
  // and then delivered to you. A both-capable provider alternates an image tile
  // vs an animated video film-strip. Parks mid-route + freezes under rm.
  let tspd = 0.075;   // constant — see the OUTBOUND note on why speed isn't boost-scaled
  let tRaw = mt * tspd + hash1(spokeId * 5.41 + 2.0);
  let tFrac = select(fract(tRaw), 0.30, rm > 0.5);
  if (tFrac < 0.5) {
    let tCyc = select(floor(tRaw), 0.0, rm > 0.5);
    let isVid = pickOutput(i32(outMask), hash1(spokeId * 9.7 + tCyc * 2.3)) == 1;
    let ph = tFrac / 0.5;
    let center = routePos(app, gate, model, c, 1.0 - ph);   // model (ph=0) -> client (ph=1)
    let q = p - center;
    if (dot(q, q) < 0.012) {
      let hsz = 0.012 + 0.011 * ph + 0.006 * boost;         // resolves larger as it arrives
      let sd = sdRoundBox(q, vec2<f32>(hsz, hsz), hsz * 0.28);
      let frame = smoothstep(0.0026, 0.0, abs(sd)) * 0.75;
      let glow = smoothstep(0.04, 0.0, sd);
      let inBox = smoothstep(0.0018, 0.0, sd);
      let lx = q.x / hsz;
      let ly = q.y / hsz;
      var content = 0.0;
      if (isVid) {
        // Animated film-strip: scrolling sprocket holes, drifting content bands,
        // and a playhead sweeping left->right (freezes to a frame under rm).
        let vt = mt * 1.6;
        let edgeBand = step(0.60, abs(ly));
        let holes = smoothstep(0.16, 0.0, abs(fract(lx * 2.5 + vt) - 0.5)) * edgeBand;
        let win = step(abs(ly), 0.50) * step(abs(lx), 0.92);
        let bands = (0.42 + 0.58 * sin((lx * 3.0 - vt * 2.2) * 3.14159265)) * win;
        let playX = -0.85 + 1.7 * fract(vt * 0.55);
        let playhead = smoothstep(0.10, 0.0, abs(lx - playX)) * step(abs(ly), 0.56);
        content = inBox * (bands * 0.65 + holes * 0.9 + playhead * 1.1);
      } else {
        // Still image: a shimmering framed picture.
        let shimmer = 0.72 + 0.28 * sin((q.x - q.y) * 150.0 + mt * 7.0);
        content = inBox * 0.62 * shimmer;
      }
      let life = smoothstep(0.0, 0.10, ph) * smoothstep(0.0, 0.16, 1.0 - ph);
      let hue = ph * 1.15 + (q.x + q.y) * 1.4;
      let irid = 0.5 + 0.5 * cos(6.2831853 * (vec3<f32>(1.0, 0.85, 0.7) * hue + vec3<f32>(0.0, 0.25, 0.5)));
      let baseTint = select(imgTint, vidTint, isVid);
      let media0 = mix(baseTint, irid, 0.42);
      let lum = dot(media0, vec3<f32>(0.3, 0.59, 0.11));
      let media = clamp(vec3<f32>(lum) + (media0 - vec3<f32>(lum)) * 1.8, vec3<f32>(0.0), vec3<f32>(1.3));
      let vibrancy = 0.5 + 0.75 * ph;
      let tile = (content + frame + glow * 0.45) * (0.55 + 0.7 * boost) * life * vibrancy;
      col = col + media * tile;
    }
  }
  return col;
}

// Rounded-box halo: a soft outline + bloom that follows the node's actual
// rectangle (ext = half-width/half-height in p-space), so wide cards get a
// card-shaped glow rather than a circle that pokes out past their short edge.
fn halo(p : vec2<f32>, c : vec2<f32>, ext : vec2<f32>, foc : f32, tint : vec3<f32>) -> vec3<f32> {
  let q = p - c;
  let corner = min(ext.x, ext.y) * 0.45;
  let sd = sdRoundBox(q, ext, corner);
  let ring = smoothstep(0.018, 0.0, abs(sd));
  let bloom = (1.0 - smoothstep(0.0, 0.18, max(sd, 0.0))) * (0.06 + 0.52 * foc);
  return tint * (ring * (0.20 + 1.05 * foc) + bloom);
}

@fragment
fn fs(in : VSOut) -> @location(0) vec4<f32> {
  let res = u.res.xy;
  let count = i32(u.res.w);
  let time = u.ctl.x;
  let rm = u.ctl.y;
  let aspect = res.x / res.y;
  let ax = vec2<f32>(aspect, 1.0);

  let uv = in.uv;
  let p0 = uv * ax;
  let mouse = u.mouse.xy * ax;

  // ---- background flow field ----
  let ft = time * (1.0 - rm);
  let warp = fbm(p0 * 3.0 + vec2<f32>(ft * 0.05, ft * 0.03));
  var bg = fbm(p0 * 2.0 + warp * 0.6 + vec2<f32>(-ft * 0.02, 0.0));
  bg = pow(clamp(bg, 0.0, 1.0), 2.4);
  var col = vec3<f32>(0.043, 0.047, 0.071);     // panel base (matches CSS)
  col = col + vec3<f32>(0.10, 0.09, 0.26) * bg * 0.5;

  // cursor light
  let md = length(p0 - mouse);
  col = col + vec3<f32>(0.30, 0.24, 0.60) * smoothstep(0.4, 0.0, md) * 0.12 * u.mouse.z * (1.0 - rm * 0.6);

  let app  = u.geom[0].xy * ax;
  let gate = u.geom[1].xy * ax;
  let appFoc = u.geom[0].w;
  let gateFoc = u.geom[1].w;

  // ---- cursor gravitational lensing ----
  // Near the pointer, evaluate the light-paths (spokes + media) at a slightly
  // displaced sample point so the traces appear to bow around the cursor. Halos
  // and the hub are sampled at the true p0 so the labels' glow never drifts.
  let toCur = p0 - mouse;
  let cdist = length(toCur);
  let lensAmt = smoothstep(0.18, 0.0, cdist) * 0.02 * u.mouse.z * (1.0 - rm);
  let p = p0 + (toCur / max(cdist, 1.0e-4)) * lensAmt;

  // ---- rotating conic scan beam ----
  // A slow wedge of light sweeping around the hub (period ~9s). It lifts the
  // spoke it currently crosses (via the per-spoke beam term below) and lays a
  // faint wash over the background — an always-on sign of live per-pixel work
  // no static SVG can fake. Frozen at a meaningful angle under reduced motion.
  let TAU = 6.2831853;
  let scanAngle = select(time * TAU / 9.0, -0.785, rm > 0.5);
  let beamDir = vec2<f32>(cos(scanAngle), sin(scanAngle));
  let pixDir = (p0 - gate) / max(length(p0 - gate), 1.0e-4);
  let beamWedge = smoothstep(0.86, 1.0, dot(beamDir, pixDir));
  let beamFall = smoothstep(0.95, 0.06, length(p0 - gate));
  let scan = beamWedge * beamFall;

  // Saturated media-kind colours — hot magenta for image, electric cyan for
  // video. Kept pure (low minimum channel) so the returning tiles stay vivid.
  let imgTint = vec3<f32>(1.0, 0.30, 0.85);
  let vidTint = vec3<f32>(0.18, 0.80, 1.0);

  // app <-> gateway trunk (straight; the prompt's way in), lit whenever any
  // route downstream is active.
  var anyProv = 0.0;
  for (var i = 2; i < count; i = i + 1) {
    anyProv = max(anyProv, u.geom[i].w);
  }
  let agBoost = max(max(appFoc, gateFoc), anyProv);
  // The client<->gateway trunk is just the glowing artery; its payloads are the
  // per-provider packets below, which run the FULL route through it — so every
  // request starts at the client and every result returns to it.
  col = col + trace(p, app, gate, agBoost, 0.0);

  // gateway -> each provider: the bowed spoke trace, plus packets routed end to
  // end (client -> gateway -> model, and the result model -> gateway -> client).
  // The conic beam lifts the spoke it is sweeping; hover dominates via max().
  for (var i = 2; i < count; i = i + 1) {
    let pc = u.geom[i].xy * ax;
    let foc = u.geom[i].w;
    let vocab = i32(u.nodeFlags[i].x);
    let outMask = u.nodeFlags[i].y;
    let sdir = (pc - gate) / max(length(pc - gate), 1.0e-4);
    let spokeBeam = smoothstep(0.985, 1.0, dot(beamDir, sdir));
    let boost = max(max(foc, gateFoc * 0.8), spokeBeam * 0.5);
    col = col + trace(p, gate, pc, boost, 0.12);
    col = col + packets(p, app, gate, pc, time + f32(i) * 1.3, boost, rm, vocab, f32(i), outMask);
  }

  // faint violet wash riding the scan wedge
  col = col + vec3<f32>(0.40, 0.34, 0.95) * scan * 0.05;

  // ---- gateway hub core (soft, pulsing; never competes with the media) ----
  let activeRoute = max(anyProv, gateFoc);
  let gd = length(p0 - gate);
  let core = vec3<f32>(0.46, 0.36, 1.0) * smoothstep(0.17, 0.0, gd) * (0.05 + 0.10 * activeRoute);
  let pulse = 0.5 + 0.5 * sin(time * 5.0 * (1.0 - rm));
  let hubRing = smoothstep(0.006, 0.0, abs(gd - 0.045)) * (0.10 + 0.22 * pulse);
  col = col + vec3<f32>(0.50, 0.40, 1.0) * hubRing + core;

  // ---- node halos (rounded-box, sized from each node's real rect) ----
  // Looped over every slot, so ALL providers light up — not just the first few.
  let violet = vec3<f32>(0.50, 0.40, 1.0);
  let appExt = vec2<f32>(u.nodeMeta[0].z * aspect, u.nodeMeta[0].w);
  let gateExt = vec2<f32>(u.nodeMeta[1].z * aspect, u.nodeMeta[1].w);
  col = col + halo(p0, app, appExt, appFoc, vec3<f32>(0.55, 0.70, 1.0));
  col = col + halo(p0, gate, gateExt, gateFoc, violet);
  for (var i = 2; i < count; i = i + 1) {
    let pc = u.geom[i].xy * ax;
    let kind = u.nodeMeta[i].y;
    let ext = vec2<f32>(u.nodeMeta[i].z * aspect, u.nodeMeta[i].w);
    let tint = mix(imgTint, vidTint, kind);
    // the sweeping beam gives a passing provider a brief extra bloom
    let sdir = (pc - gate) / max(length(pc - gate), 1.0e-4);
    let spokeBeam = smoothstep(0.985, 1.0, dot(beamDir, sdir));
    col = col + halo(p0, pc, ext, max(u.geom[i].w, spokeBeam * 0.35), tint);
  }

  // vignette + tone
  let vd = length(uv - vec2<f32>(0.5, 0.5));
  col = col * (1.0 - 0.55 * smoothstep(0.35, 0.95, vd));
  col = col / (col + vec3<f32>(1.0));          // reinhard
  col = pow(col, vec3<f32>(0.85));             // lift

  return vec4<f32>(col, 1.0);
}
`;
