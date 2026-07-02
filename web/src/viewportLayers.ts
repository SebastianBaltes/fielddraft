/**
 * three.js layer convention of the viewport.
 *
 * Layer 0 is the plain scene (the classic-WebGL path renders it directly).
 * The SDF render paths draw the body via their own passes and then composite
 * the scene's overlay layers on top — see `renderOverlayLayer` in
 * viewportSdfOverlays.tsx.
 */

/** 3D overlays (B-Rep edges, sketches, gizmos, selection) MUST enable this
 *  layer: the SDF paths render ONLY DATUM/OVERLAY layers over the body —
 *  plain layer 0 is invisible there (in WebGL it still shows → tückisch). */
export const OVERLAY_LAYER = 1;

// Datum planes / origin live on their OWN layer so the SDF path can re-draw
// them AFTER the body shade pass but BEFORE the x-ray depth-clear that lets
// B-Rep edges shine through solids. That way datums share the bodies' depth
// buffer (a solid in front occludes the datum behind it — ticket 1m8g) while
// edges keep their see-through x-ray behaviour. Datums also stay on layer 0 so
// the plain WebGL path keeps drawing them with no extra orchestration.
export const DATUM_LAYER = 2;
