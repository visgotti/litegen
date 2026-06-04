/**
 * WebGL2 (GLSL ES 3.00) port of the infrastructure-flow shader in shader.ts.
 *
 * This is a faithful, line-for-line translation of FLOW_WGSL so the WebGL2
 * fallback is pixel-identical to the WebGPU path. The uniform block `U` uses the
 * SAME std140 layout as the WGSL struct, so renderer.ts packs one Float32Array
 * that feeds either backend unchanged. If you edit one shader, edit the other.
 *
 * Notes on the WGSL→GLSL mapping:
 *   - `select(a, b, cond)`  →  `cond ? b : a`
 *   - `vecN<f32>`           →  `vecN`,  `array<T, n>` → `T[n]`
 *   - i32 bitwise masks are why this targets WebGL2/ES 3.00 (ES 1.00 can't).
 */

/** Fullscreen triangle; uv is 0..1 y-down to match the DOM (as in the WGSL). */
export const FLOW_GLSL_VS = /* glsl */ `#version 300 es
out vec2 vUv;
const vec2 P[3] = vec2[3](vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
void main() {
  vec2 xy = P[gl_VertexID];
  gl_Position = vec4(xy, 0.0, 1.0);
  vUv = vec2(xy.x * 0.5 + 0.5, 1.0 - (xy.y * 0.5 + 0.5));
}
`;

export const FLOW_GLSL_FS = /* glsl */ `#version 300 es
precision highp float;
precision highp int;

layout(std140) uniform U {
  vec4 res;            // (canvas.x, canvas.y, dpr, nodeCount)
  vec4 ctl;            // (time seconds, reducedMotion 0|1, pad, pad)
  vec4 mouse;          // (pointer.x uv, pointer.y uv, inside 0|1, pad)
  vec4 geom[24];       // per node (centerX uv, centerY uv, radius uv-x, focus 0..1)
  vec4 nodeMeta[24];   // per node (role, kind, halfW/stageW, halfH/stageH)
  vec4 nodeFlags[24];  // per node (.x input bitmask, .y output bitmask)
};

in vec2 vUv;
out vec4 fragColor;

// ---- noise -----------------------------------------------------------------
float hash21(vec2 p) {
  float h = dot(p, vec2(127.1, 311.7));
  return fract(sin(h) * 43758.5453123);
}
float noise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  float a = hash21(i);
  float b = hash21(i + vec2(1.0, 0.0));
  float c = hash21(i + vec2(0.0, 1.0));
  float d = hash21(i + vec2(1.0, 1.0));
  vec2 uu = f * f * (3.0 - 2.0 * f);
  return mix(mix(a, b, uu.x), mix(c, d, uu.x), uu.y);
}
float fbm(vec2 p0) {
  vec2 p = p0;
  float v = 0.0;
  float amp = 0.5;
  for (int i = 0; i < 5; i++) {
    v = v + amp * noise(p);
    p = p * 2.02;
    amp = amp * 0.5;
  }
  return v;
}

vec2 segInfo(vec2 p, vec2 a, vec2 b) {
  vec2 pa = p - a;
  vec2 ba = b - a;
  float h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
  float d = length(pa - ba * h);
  return vec2(d, h);
}

float sdRoundBox(vec2 p, vec2 b, float r) {
  vec2 q = abs(p) - b + vec2(r);
  return min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0))) - r;
}

vec2 bez(vec2 a, vec2 c, vec2 b, float t) {
  float s = 1.0 - t;
  return s * s * a + 2.0 * s * t * c + t * t * b;
}

float hash1(float n) {
  return fract(sin(n * 12.9898) * 43758.5453123);
}

int pickVocab(int mask, float r) {
  int opts[3] = int[3](0, 0, 0);
  int n = 0;
  if ((mask & 1) != 0) { opts[n] = 0; n = n + 1; }
  if ((mask & 2) != 0) { opts[n] = 1; n = n + 1; }
  if ((mask & 4) != 0) { opts[n] = 2; n = n + 1; }
  if (n == 0) { return 0; }
  int idx = clamp(int(floor(r * float(n))), 0, n - 1);
  return opts[idx];
}

int pickOutput(int mask, float r) {
  int opts[2] = int[2](0, 0);
  int n = 0;
  if ((mask & 1) != 0) { opts[n] = 0; n = n + 1; }
  if ((mask & 2) != 0) { opts[n] = 1; n = n + 1; }
  if (n == 0) { return 0; }
  int idx = clamp(int(floor(r * float(n))), 0, n - 1);
  return opts[idx];
}

vec3 glyph(vec2 q, int kind, float boost) {
  vec3 cool = vec3(0.55, 0.62, 0.95);
  float amp = 0.45 + 0.55 * boost;
  if (kind == 0) {
    float sd = sdRoundBox(q, vec2(0.013, 0.010), 0.004);
    float frame = smoothstep(0.0022, 0.0, abs(sd)) * 0.6;
    float lines = 0.0;
    for (int k = 0; k < 2; k++) {
      float yy = -0.004 + float(k) * 0.008;
      float wob = 0.0016 * sin(q.x * 420.0);
      lines = lines + smoothstep(0.0014, 0.0, abs(q.y - yy - wob)) * step(abs(q.x), 0.009);
    }
    return cool * (frame + lines * 0.5) * amp;
  }
  if (kind == 1) {
    float sd = sdRoundBox(q, vec2(0.011, 0.011), 0.003);
    float body = smoothstep(0.001, 0.0, sd) * 0.32;
    float frame = smoothstep(0.0022, 0.0, abs(sd)) * 0.72;
    return cool * (body + frame) * amp;
  }
  float acc = 0.0;
  for (int k = 0; k < 2; k++) {
    vec2 off = vec2(-0.004 + float(k) * 0.008, 0.004 - float(k) * 0.008);
    float sd = sdRoundBox(q - off, vec2(0.0075, 0.0075), 0.0025);
    acc = acc + smoothstep(0.0019, 0.0, abs(sd)) * 0.7 + smoothstep(0.0009, 0.0, sd) * 0.22;
  }
  return cool * acc * amp;
}

vec2 curveDist(vec2 p, vec2 a, vec2 c, vec2 b) {
  vec2 prev = a;
  float bestD = 1.0e9;
  float bestAlong = 0.0;
  for (int k = 0; k < 3; k++) {
    float t1 = float(k + 1) / 3.0;
    vec2 pt = bez(a, c, b, t1);
    vec2 inf = segInfo(p, prev, pt);
    if (inf.x < bestD) {
      bestD = inf.x;
      bestAlong = (float(k) + inf.y) / 3.0;
    }
    prev = pt;
  }
  return vec2(bestD, bestAlong);
}

vec3 trace(vec2 p, vec2 a, vec2 b, float boost, float bowScale) {
  vec2 dir = b - a;
  float len = length(dir);
  vec2 perp = normalize(vec2(-dir.y, dir.x));
  vec2 c = (a + b) * 0.5 + perp * (len * bowScale);
  float d = curveDist(p, a, c, b).x;
  vec3 violet = vec3(0.46, 0.36, 1.0);
  vec3 col = vec3(0.0);
  col = col + violet * smoothstep(0.0026, 0.0, d) * (0.26 + 0.55 * boost);
  col = col + violet * smoothstep(0.055, 0.0, d) * (0.045 + 0.34 * boost);
  return col;
}

vec2 routePos(vec2 app, vec2 gate, vec2 model, vec2 c, float t) {
  float SPLIT = 0.30;
  if (t < SPLIT) {
    return mix(app, gate, t / SPLIT);
  }
  return bez(gate, c, model, (t - SPLIT) / (1.0 - SPLIT));
}

vec3 packets(vec2 p, vec2 app, vec2 gate, vec2 model,
             float mt, float boost, float rm, int vocab, float spokeId, float outMask) {
  vec3 col = vec3(0.0);
  vec2 dir = model - gate;
  float len = length(dir);
  vec2 perp = normalize(vec2(-dir.y, dir.x));
  vec2 c = (gate + model) * 0.5 + perp * (len * 0.12);
  vec3 imgTint = vec3(1.0, 0.30, 0.85);
  vec3 vidTint = vec3(0.18, 0.80, 1.0);

  // OUTBOUND request: client -> gateway -> model.
  float ospd = 0.10 + 0.09 * boost;
  float oRaw = mt * ospd + hash1(spokeId * 3.17 + 1.0);
  float oFrac = rm > 0.5 ? 0.25 : fract(oRaw);
  if (oFrac < 0.45) {
    float oCyc = rm > 0.5 ? 0.0 : floor(oRaw);
    float ph = oFrac / 0.45;
    int kindSel = pickVocab(vocab, hash1(spokeId * 7.13 + oCyc * 3.7));
    vec2 center = routePos(app, gate, model, c, ph);
    vec2 qo = p - center;
    if (dot(qo, qo) < 0.0026) {
      float life = smoothstep(0.0, 0.10, ph) * smoothstep(0.0, 0.16, 1.0 - ph);
      col = col + glyph(qo, kindSel, boost) * life;
    }
  }

  // INBOUND result: model -> gateway -> client.
  float tspd = 0.075 + 0.10 * boost;
  float tRaw = mt * tspd + hash1(spokeId * 5.41 + 2.0);
  float tFrac = rm > 0.5 ? 0.30 : fract(tRaw);
  if (tFrac < 0.5) {
    float tCyc = rm > 0.5 ? 0.0 : floor(tRaw);
    bool isVid = pickOutput(int(outMask), hash1(spokeId * 9.7 + tCyc * 2.3)) == 1;
    float ph = tFrac / 0.5;
    vec2 center = routePos(app, gate, model, c, 1.0 - ph);
    vec2 q = p - center;
    if (dot(q, q) < 0.012) {
      float hsz = 0.012 + 0.011 * ph + 0.006 * boost;
      float sd = sdRoundBox(q, vec2(hsz, hsz), hsz * 0.28);
      float frame = smoothstep(0.0026, 0.0, abs(sd)) * 0.75;
      float glow = smoothstep(0.04, 0.0, sd);
      float inBox = smoothstep(0.0018, 0.0, sd);
      float lx = q.x / hsz;
      float ly = q.y / hsz;
      float content = 0.0;
      if (isVid) {
        float vt = mt * 1.6;
        float edgeBand = step(0.60, abs(ly));
        float holes = smoothstep(0.16, 0.0, abs(fract(lx * 2.5 + vt) - 0.5)) * edgeBand;
        float win = step(abs(ly), 0.50) * step(abs(lx), 0.92);
        float bands = (0.42 + 0.58 * sin((lx * 3.0 - vt * 2.2) * 3.14159265)) * win;
        float playX = -0.85 + 1.7 * fract(vt * 0.55);
        float playhead = smoothstep(0.10, 0.0, abs(lx - playX)) * step(abs(ly), 0.56);
        content = inBox * (bands * 0.65 + holes * 0.9 + playhead * 1.1);
      } else {
        float shimmer = 0.72 + 0.28 * sin((q.x - q.y) * 150.0 + mt * 7.0);
        content = inBox * 0.62 * shimmer;
      }
      float life = smoothstep(0.0, 0.10, ph) * smoothstep(0.0, 0.16, 1.0 - ph);
      float hue = ph * 1.15 + (q.x + q.y) * 1.4;
      vec3 irid = 0.5 + 0.5 * cos(6.2831853 * (vec3(1.0, 0.85, 0.7) * hue + vec3(0.0, 0.25, 0.5)));
      vec3 baseTint = isVid ? vidTint : imgTint;
      vec3 media0 = mix(baseTint, irid, 0.42);
      float lum = dot(media0, vec3(0.3, 0.59, 0.11));
      vec3 media = clamp(vec3(lum) + (media0 - vec3(lum)) * 1.8, vec3(0.0), vec3(1.3));
      float vibrancy = 0.5 + 0.75 * ph;
      float tile = (content + frame + glow * 0.45) * (0.55 + 0.7 * boost) * life * vibrancy;
      col = col + media * tile;
    }
  }
  return col;
}

vec3 halo(vec2 p, vec2 c, vec2 ext, float foc, vec3 tint) {
  vec2 q = p - c;
  float corner = min(ext.x, ext.y) * 0.45;
  float sd = sdRoundBox(q, ext, corner);
  float ring = smoothstep(0.018, 0.0, abs(sd));
  float bloom = (1.0 - smoothstep(0.0, 0.18, max(sd, 0.0))) * (0.06 + 0.52 * foc);
  return tint * (ring * (0.20 + 1.05 * foc) + bloom);
}

void main() {
  vec2 res2 = res.xy;
  int count = int(res.w);
  float time = ctl.x;
  float rm = ctl.y;
  float aspect = res2.x / res2.y;
  vec2 ax = vec2(aspect, 1.0);

  vec2 uv = vUv;
  vec2 p0 = uv * ax;
  vec2 mousep = mouse.xy * ax;

  // ---- background flow field ----
  float ft = time * (1.0 - rm);
  float warp = fbm(p0 * 3.0 + vec2(ft * 0.05, ft * 0.03));
  float bg = fbm(p0 * 2.0 + warp * 0.6 + vec2(-ft * 0.02, 0.0));
  bg = pow(clamp(bg, 0.0, 1.0), 2.4);
  vec3 col = vec3(0.043, 0.047, 0.071);
  col = col + vec3(0.10, 0.09, 0.26) * bg * 0.5;

  float md = length(p0 - mousep);
  col = col + vec3(0.30, 0.24, 0.60) * smoothstep(0.4, 0.0, md) * 0.12 * mouse.z * (1.0 - rm * 0.6);

  vec2 app = geom[0].xy * ax;
  vec2 gate = geom[1].xy * ax;
  float appFoc = geom[0].w;
  float gateFoc = geom[1].w;

  // ---- cursor gravitational lensing ----
  vec2 toCur = p0 - mousep;
  float cdist = length(toCur);
  float lensAmt = smoothstep(0.18, 0.0, cdist) * 0.02 * mouse.z * (1.0 - rm);
  vec2 p = p0 + (toCur / max(cdist, 1.0e-4)) * lensAmt;

  // ---- rotating conic scan beam ----
  float TAU = 6.2831853;
  float scanAngle = rm > 0.5 ? -0.785 : time * TAU / 9.0;
  vec2 beamDir = vec2(cos(scanAngle), sin(scanAngle));
  vec2 pixDir = (p0 - gate) / max(length(p0 - gate), 1.0e-4);
  float beamWedge = smoothstep(0.86, 1.0, dot(beamDir, pixDir));
  float beamFall = smoothstep(0.95, 0.06, length(p0 - gate));
  float scan = beamWedge * beamFall;

  vec3 imgTint = vec3(1.0, 0.30, 0.85);
  vec3 vidTint = vec3(0.18, 0.80, 1.0);

  float anyProv = 0.0;
  for (int i = 2; i < count; i++) {
    anyProv = max(anyProv, geom[i].w);
  }
  float agBoost = max(max(appFoc, gateFoc), anyProv);
  col = col + trace(p, app, gate, agBoost, 0.0);

  for (int i = 2; i < count; i++) {
    vec2 pc = geom[i].xy * ax;
    float foc = geom[i].w;
    int vocab = int(nodeFlags[i].x);
    float outMask = nodeFlags[i].y;
    vec2 sdir = (pc - gate) / max(length(pc - gate), 1.0e-4);
    float spokeBeam = smoothstep(0.985, 1.0, dot(beamDir, sdir));
    float boost = max(max(foc, gateFoc * 0.8), spokeBeam * 0.5);
    col = col + trace(p, gate, pc, boost, 0.12);
    col = col + packets(p, app, gate, pc, time + float(i) * 1.3, boost, rm, vocab, float(i), outMask);
  }

  col = col + vec3(0.40, 0.34, 0.95) * scan * 0.05;

  // ---- gateway hub core ----
  float activeRoute = max(anyProv, gateFoc);
  float gd = length(p0 - gate);
  vec3 core = vec3(0.46, 0.36, 1.0) * smoothstep(0.17, 0.0, gd) * (0.05 + 0.10 * activeRoute);
  float pulse = 0.5 + 0.5 * sin(time * 5.0 * (1.0 - rm));
  float hubRing = smoothstep(0.006, 0.0, abs(gd - 0.045)) * (0.10 + 0.22 * pulse);
  col = col + vec3(0.50, 0.40, 1.0) * hubRing + core;

  // ---- node halos ----
  vec3 violet = vec3(0.50, 0.40, 1.0);
  vec2 appExt = vec2(nodeMeta[0].z * aspect, nodeMeta[0].w);
  vec2 gateExt = vec2(nodeMeta[1].z * aspect, nodeMeta[1].w);
  col = col + halo(p0, app, appExt, appFoc, vec3(0.55, 0.70, 1.0));
  col = col + halo(p0, gate, gateExt, gateFoc, violet);
  for (int i = 2; i < count; i++) {
    vec2 pc = geom[i].xy * ax;
    float kind = nodeMeta[i].y;
    vec2 ext = vec2(nodeMeta[i].z * aspect, nodeMeta[i].w);
    vec3 tint = mix(imgTint, vidTint, kind);
    vec2 sdir = (pc - gate) / max(length(pc - gate), 1.0e-4);
    float spokeBeam = smoothstep(0.985, 1.0, dot(beamDir, sdir));
    col = col + halo(p0, pc, ext, max(geom[i].w, spokeBeam * 0.35), tint);
  }

  // vignette + tone
  float vd = length(uv - vec2(0.5, 0.5));
  col = col * (1.0 - 0.55 * smoothstep(0.35, 0.95, vd));
  col = col / (col + vec3(1.0));
  col = pow(col, vec3(0.85));

  fragColor = vec4(col, 1.0);
}
`;
