'use client';

/**
 * "How it works" — the WebGPU infrastructure-flow visual.
 *
 * Layering inside the stage (back to front):
 *   1. <canvas>     — WebGPU glow: flow field, links, streaming media tiles,
 *                     node halos. Decorative, so aria-hidden.
 *   2. <svg>        — static fallback wires, shown when WebGPU is unavailable.
 *   3. .overlay     — the crisp, accessible HTML labels (app / gateway / pills /
 *                     providers). These are the source of truth for positions;
 *                     the renderer reads their real rects every frame.
 *
 * Interaction is intentionally light: a small cursor light + parallax, and a
 * per-node hover "focus" that brightens that route and spawns brighter media
 * tiles. Hover feedback that can't be done in CSS (additive bloom, faster
 * energy packets along the exact link) is what the GPU buys us; everything that
 * must stay legible (the labels, the route on hover) is plain DOM/CSS.
 *
 * Accessibility: the canvas/SVG are decorative (aria-hidden); the node names are
 * real text, and a visually-hidden <figcaption> describes the flow's topology
 * so the relationships aren't conveyed by the visuals alone.
 */
import { useEffect, useRef, useState } from 'react';
import { useTranslations } from 'next-intl';
import { Boxes, Layers } from 'lucide-react';
import {
  ALL_NODES,
  APP_NODE,
  GATEWAY_NODE,
  GATEWAY_PILLS,
  PROVIDER_NODES,
  inputsMask,
  outputsMask,
  type FlowNode,
} from './infra-flow/nodes';
import { PROVIDER_MODELS } from './infra-flow/provider-models.generated';
import type { FlowHandle, FlowRenderer, PointerState } from './infra-flow/renderer';
import styles from './InfraFlow.module.css';

// Mirror next.config's basePath so logo URLs resolve under a subpath deploy
// (GitHub Pages) as well as at the domain root (Cloudflare).
const BASE = process.env.NEXT_PUBLIC_BASE_PATH?.trim().replace(/\/$/, '') || '';

/** Build the CSS custom properties that position a node in both layouts. */
function posVars(node: FlowNode): React.CSSProperties {
  return {
    '--x': `${node.pos.x}%`,
    '--y': `${node.pos.y}%`,
    '--mx': `${node.posMobile.x}%`,
    '--my': `${node.posMobile.y}%`,
  } as React.CSSProperties;
}

/**
 * When a provider is pinned, every other provider slides away from it. The
 * offset (--sx/--sy, in stage %) points away from the selected node and falls
 * off with distance, so neighbours part the most. CSS transitions the resulting
 * left/top for the fluid motion; spokes follow because the renderer re-measures
 * each node's rect every frame.
 */
function spreadVars(node: FlowNode, selected: FlowNode | null): React.CSSProperties {
  if (!selected || node.role !== 'provider' || node.id === selected.id) {
    return { '--sx': '0%', '--sy': '0%' } as React.CSSProperties;
  }
  const dx = node.pos.x - selected.pos.x;
  const dy = node.pos.y - selected.pos.y;
  const dist = Math.hypot(dx, dy) || 1;
  const push = 16 * Math.exp(-dist / 24); // % of stage; nearer nodes move more
  // Clamp so a pushed node can't leave the stage.
  const sx = Math.max(5, Math.min(95, node.pos.x + (dx / dist) * push)) - node.pos.x;
  const sy = Math.max(7, Math.min(93, node.pos.y + (dy / dist) * push)) - node.pos.y;
  return { '--sx': `${sx.toFixed(1)}%`, '--sy': `${sy.toFixed(1)}%` } as React.CSSProperties;
}

export function InfraFlow() {
  const t = useTranslations('howItWorks');

  const stageRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  // The full .node element (used to look up a node by id).
  const nodeEls = useRef<Map<string, HTMLElement>>(new Map());
  // The element the renderer should *measure* for geometry. For providers this
  // is the logo mark, not the whole label stack, so links/halos hit the logo
  // (the thing users read as "the node") rather than the center of the text.
  const anchorEls = useRef<Map<string, HTMLElement>>(new Map());

  // Live interaction state read by the renderer each frame (no re-renders).
  const pointerRef = useRef<PointerState>({ x: 0.5, y: 0.5, inside: false });
  const focusRef = useRef<Map<string, number>>(new Map());
  // Held so hover handlers can poke a redraw under reduced motion (no rAF loop).
  const rendererRef = useRef<FlowRenderer | null>(null);

  const [gpuActive, setGpuActive] = useState(false);
  // Active render backend + whether a `?renderer=` debug flag pinned it (shows a
  // small badge so WebGPU vs the WebGL2 fallback can be compared side by side).
  const [backendKind, setBackendKind] = useState<'webgpu' | 'webgl2' | 'svg' | null>(null);
  const [forcedBackend, setForcedBackend] = useState(false);
  // The currently-hovered node id, used to light its route in the DOM/SVG layer
  // so the "trace its route" interaction works even when WebGPU is unavailable.
  const [focusedId, setFocusedId] = useState<string | null>(null);
  // Initialized from matchMedia in an effect (SSR has no window).
  const [reducedMotion, setReducedMotion] = useState(false);
  // Click-to-pin a provider: expands it (model details) + pushes the others away.
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selectedIdRef = useRef<string | null>(null);

  // Track the OS reduced-motion preference and react to changes at runtime.
  useEffect(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return;
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
    setReducedMotion(mq.matches);
    const onChange = (e: MediaQueryListEvent) => setReducedMotion(e.matches);
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, []);

  // Create (and re-create on reduced-motion change) the WebGPU renderer.
  useEffect(() => {
    const stage = stageRef.current;
    const canvas = canvasRef.current;
    if (!stage || !canvas) return;

    // Debug override: ?renderer=webgl|webgpu|svg pins one backend for comparison.
    const flag =
      typeof window !== 'undefined'
        ? new URLSearchParams(window.location.search).get('renderer')
        : null;
    const force: 'webgpu' | 'webgl2' | 'svg' | undefined =
      flag === 'webgl' || flag === 'webgl2'
        ? 'webgl2'
        : flag === 'webgpu'
          ? 'webgpu'
          : flag === 'svg' || flag === 'none'
            ? 'svg'
            : undefined;
    setForcedBackend(force !== undefined);

    // Assemble handles in the order the shader expects: app, gateway, providers.
    // Each handle's `el` is the element to measure (anchor if one was provided).
    const handles: FlowHandle[] = [];
    for (const node of ALL_NODES) {
      const measureEl = anchorEls.current.get(node.id) ?? nodeEls.current.get(node.id);
      if (!measureEl) return; // refs not ready yet; skip this run
      handles.push({
        id: node.id,
        el: measureEl,
        role: node.role,
        kind: node.kind,
        inputsMask: inputsMask(node),
        outputsMask: outputsMask(node),
      });
    }

    const controller = new AbortController();
    let renderer: FlowRenderer | null = null;

    import('./infra-flow/renderer')
      .then(async ({ createFlowRenderer }) => {
        if (controller.signal.aborted) return;
        renderer = await createFlowRenderer({
          canvas,
          stage,
          nodes: handles,
          pointer: pointerRef.current,
          focus: focusRef.current,
          reducedMotion,
          signal: controller.signal,
          forceBackend: force,
          // Unexpected device loss → reveal the static SVG fallback.
          onLost: () => setGpuActive(false),
        });
        if (controller.signal.aborted) {
          renderer?.destroy();
          renderer = null;
          return;
        }
        rendererRef.current = renderer;
        setGpuActive(Boolean(renderer));
        setBackendKind(renderer ? renderer.kind : 'svg');
      })
      .catch(() => {
        /* keep the SVG fallback */
      });

    return () => {
      controller.abort();
      renderer?.destroy();
      rendererRef.current = null;
      setGpuActive(false);
    };
  }, [reducedMotion]);

  const handlePointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    pointerRef.current.x = (e.clientX - r.left) / r.width;
    pointerRef.current.y = (e.clientY - r.top) / r.height;
    pointerRef.current.inside = true;
  };
  const handlePointerLeave = () => {
    pointerRef.current.inside = false;
  };

  const setFocus = (id: string, v: number) => {
    // A pinned (selected) node stays lit even when the pointer leaves it.
    const next = v < 1 && selectedIdRef.current === id ? 1 : v;
    focusRef.current.set(id, next);
    // Drive the DOM/SVG highlight (route + dimmed siblings) — independent of GPU.
    setFocusedId((cur) => (next >= 1 ? id : cur === id ? null : cur));
    // Under reduced motion there's no animation loop, so nudge a single repaint
    // to reflect the hover; under normal motion this is a no-op.
    rendererRef.current?.requestRedraw();
  };

  // The pinned provider node (null when nothing is selected), used to expand it
  // and to compute how far every other node parts from it.
  const selectedNode = selectedId
    ? PROVIDER_NODES.find((n) => n.id === selectedId) ?? null
    : null;
  const toggleSelect = (id: string) =>
    setSelectedId((cur) => (cur === id ? null : id));

  // Mirror selection into a ref + keep the pinned node's route lit in the GPU layer.
  useEffect(() => {
    selectedIdRef.current = selectedId;
    if (selectedId) focusRef.current.set(selectedId, 1);
    rendererRef.current?.requestRedraw();
  }, [selectedId]);

  // Element collectors (callback refs).
  const collect = (id: string) => (el: HTMLElement | null) => {
    if (el) nodeEls.current.set(id, el);
    else nodeEls.current.delete(id);
  };
  const collectAnchor = (id: string) => (el: HTMLElement | null) => {
    if (el) anchorEls.current.set(id, el);
    else anchorEls.current.delete(id);
  };

  // Fallback wires, computed from the structural node positions for both layouts.
  const links = [
    [APP_NODE, GATEWAY_NODE],
    ...PROVIDER_NODES.map((p) => [GATEWAY_NODE, p]),
  ] as const;

  return (
    <section id="how-it-works" className={styles.section}>
      <div className="container">
        <header className={styles.head}>
          <h2 className={styles.heading}>{t('heading')}</h2>
          <p className={styles.sub}>{t('subheading')}</p>
        </header>

        <figure className={styles.figure}>
          <div
            ref={stageRef}
            className={styles.stage}
            data-gpu={gpuActive ? 'on' : 'off'}
            data-focus={focusedId ?? ''}
            onPointerMove={handlePointerMove}
            onPointerLeave={handlePointerLeave}
            onClick={() => setSelectedId(null)}
          >
            <canvas ref={canvasRef} className={styles.canvas} aria-hidden="true" />

            {/* Static fallback wiring (visible until/unless WebGPU takes over). */}
            <svg
              className={styles.fallback}
              viewBox="0 0 100 100"
              preserveAspectRatio="none"
              aria-hidden="true"
            >
              <defs>
                <linearGradient id="infraflow-wire" x1="0" y1="0" x2="1" y2="0">
                  <stop offset="0%" stopColor="var(--accent)" />
                  <stop offset="100%" stopColor="var(--accent-2)" />
                </linearGradient>
              </defs>
              <g className={styles.wiresDesktop}>
                {links.map(([a, b]) => (
                  <line
                    key={`d-${a.id}-${b.id}`}
                    className={styles.wire}
                    data-link={a.id === 'app' ? 'app' : b.id}
                    x1={a.pos.x}
                    y1={a.pos.y}
                    x2={b.pos.x}
                    y2={b.pos.y}
                    stroke="url(#infraflow-wire)"
                  />
                ))}
              </g>
              <g className={styles.wiresMobile}>
                {links.map(([a, b]) => (
                  <line
                    key={`m-${a.id}-${b.id}`}
                    className={styles.wire}
                    data-link={a.id === 'app' ? 'app' : b.id}
                    x1={a.posMobile.x}
                    y1={a.posMobile.y}
                    x2={b.posMobile.x}
                    y2={b.posMobile.y}
                    stroke="url(#infraflow-wire)"
                  />
                ))}
              </g>
            </svg>

            {/* Accessible, crisp labels. */}
            <div className={styles.overlay}>
              {/* App / client */}
              <div
                ref={collect(APP_NODE.id)}
                className={`${styles.node} ${styles.appNode}`}
                data-node={APP_NODE.id}
                style={posVars(APP_NODE)}
                onPointerEnter={() => setFocus(APP_NODE.id, 1)}
                onPointerLeave={() => setFocus(APP_NODE.id, 0)}
              >
                <Boxes className={styles.appIcon} size={26} aria-hidden="true" />
                <span className={styles.appLabel}>{t('app')}</span>
              </div>

              {/* Gateway */}
              <div
                ref={collect(GATEWAY_NODE.id)}
                className={`${styles.node} ${styles.gateway}`}
                data-node={GATEWAY_NODE.id}
                style={posVars(GATEWAY_NODE)}
                onPointerEnter={() => setFocus(GATEWAY_NODE.id, 1)}
                onPointerLeave={() => setFocus(GATEWAY_NODE.id, 0)}
              >
                <div className={styles.brand}>
                  <span className={styles.brandMark} aria-hidden="true">
                    <Layers size={15} />
                  </span>
                  LiteGen
                </div>
                <ul className={styles.pills} aria-label={t('pillsLabel')}>
                  {GATEWAY_PILLS.map((pill) => (
                    <li key={pill} className={styles.pill}>
                      {t(`pills.${pill}`)}
                    </li>
                  ))}
                </ul>
              </div>

              {/* Providers */}
              <ul className={styles.providerList} aria-label={t('providersLabel')}>
                {PROVIDER_NODES.map((provider) => {
                  const isSel = selectedId === provider.id;
                  const models = PROVIDER_MODELS[provider.id];
                  return (
                  <li
                    key={provider.id}
                    ref={collect(provider.id)}
                    className={`${styles.node} ${styles.provider}`}
                    data-node={provider.id}
                    data-selected={isSel ? 'true' : undefined}
                    data-side={provider.pos.y > 56 ? 'up' : 'down'}
                    style={{ ...posVars(provider), ...spreadVars(provider, selectedNode) }}
                    role="button"
                    tabIndex={0}
                    aria-expanded={isSel}
                    aria-label={provider.name}
                    onPointerEnter={() => setFocus(provider.id, 1)}
                    onPointerLeave={() => setFocus(provider.id, 0)}
                    onClick={(e) => {
                      e.stopPropagation();
                      toggleSelect(provider.id);
                    }}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' || e.key === ' ') {
                        e.preventDefault();
                        toggleSelect(provider.id);
                      } else if (e.key === 'Escape') {
                        setSelectedId(null);
                      }
                    }}
                  >
                    <span
                      ref={collectAnchor(provider.id)}
                      className={`${styles.pmark}${provider.logo ? ` ${styles.pmarkLogo}` : ''}`}
                      aria-hidden="true"
                    >
                      {provider.logo ? (
                        // eslint-disable-next-line @next/next/no-img-element
                        <img src={`${BASE}/logos/${provider.logo}`} alt="" decoding="async" />
                      ) : (
                        provider.initial
                      )}
                    </span>
                    <span className={styles.pname}>{provider.name}</span>
                    {(provider.outputs?.image || provider.outputs?.video) && (
                      <span className={styles.pkinds}>
                        {provider.outputs?.image && (
                          <span className={styles.pkind} data-kind="image">
                            {t('kinds.image')}
                          </span>
                        )}
                        {provider.outputs?.video && (
                          <span className={styles.pkind} data-kind="video">
                            {t('kinds.video')}
                          </span>
                        )}
                      </span>
                    )}
                    {isSel && models && (
                      <div
                        className={styles.panel}
                        role="group"
                        aria-label={provider.name}
                        onClick={(e) => e.stopPropagation()}
                      >
                        {models.image.length > 0 && (
                          <div className={styles.panelGroup}>
                            <span className={styles.panelHead} data-kind="image">
                              {t('kinds.image')}
                            </span>
                            <ul className={styles.panelList}>
                              {models.image.map((m) => (
                                <li key={m}>{m}</li>
                              ))}
                            </ul>
                          </div>
                        )}
                        {models.video.length > 0 && (
                          <div className={styles.panelGroup}>
                            <span className={styles.panelHead} data-kind="video">
                              {t('kinds.video')}
                            </span>
                            <ul className={styles.panelList}>
                              {models.video.map((m) => (
                                <li key={m}>{m}</li>
                              ))}
                            </ul>
                          </div>
                        )}
                      </div>
                    )}
                  </li>
                  );
                })}
              </ul>
            </div>

            {forcedBackend && (
              <div className={styles.badge} aria-hidden="true">
                {backendKind === 'webgpu'
                  ? 'WebGPU'
                  : backendKind === 'webgl2'
                    ? 'WebGL2'
                    : 'SVG fallback'}
              </div>
            )}
          </div>

          <figcaption className={styles.srOnly}>{t('figcaption')}</figcaption>
        </figure>
      </div>
    </section>
  );
}
