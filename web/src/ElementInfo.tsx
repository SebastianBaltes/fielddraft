/**
 * Element-info readout (chunk 3a).
 *
 * Renders a read-only summary of the currently-selected B-Rep element
 * (`state.selectedElement`) in the Outliner: its kind, world-space geometry,
 * and the canonical element-query string that bridges to the scripting
 * workflow. No actions yet — context-actions land in chunk 3b.
 *
 * All data is resolved straight from the kernel by scanning bodies for the
 * matching `idHash`, folding body-local geometry through `bodyPlacement` into
 * world space via `localToWorld`.
 */

import { useMemo } from "react";
import type { KernelStore, StoreState } from "./store";
import {
  localToWorld,
  type BRepEdge,
  type BRepFace,
  type BRepVertex,
  type Placement,
} from "./kernel";
import {
  elementQueryString,
  type PickedElement,
} from "./elementQuery";
import { resolveFaceWorldGeometry } from "./faceGeometry";

/** German label for each element kind. */
const KIND_LABEL: Record<PickedElement["kind"], string> = {
  face: "Fläche",
  edge: "Kante",
  vertex: "Ecke",
};

/** German label for each analytic surface kind. */
const SURFACE_LABEL: Record<BRepFace["surfaceKind"], string> = {
  plane: "Ebene",
  cylinder: "Zylinder",
  sphere: "Kugel",
  cone: "Kegel",
  torus: "Torus",
  tessellated: "Tesselliert",
};

function fmt(n: number): string {
  return n.toFixed(2);
}

function Vec({ v }: { v: [number, number, number] }) {
  return (
    <span className="vec">
      {fmt(v[0])}, {fmt(v[1])}, {fmt(v[2])}
    </span>
  );
}

type Resolved =
  | { kind: "face"; surface: string; centroid: [number, number, number]; normal: [number, number, number] }
  | { kind: "edge"; length: number; start: [number, number, number]; end: [number, number, number] }
  | { kind: "vertex"; position: [number, number, number] }
  | null;

function resolveElement(store: KernelStore, el: PickedElement): Resolved {
  const k = store.kernel;
  const count = k.bodyCount();

  if (el.kind === "face") {
    const geom = resolveFaceWorldGeometry(k, el.idHash);
    if (!geom) return null;
    return {
      kind: "face",
      surface: SURFACE_LABEL[geom.surfaceKind] ?? geom.surfaceKind,
      centroid: geom.centroid,
      normal: geom.normal,
    };
  }

  if (el.kind === "edge") {
    for (let i = 0; i < count; i++) {
      let edges: BRepEdge[];
      try {
        edges = k.bodyBrepEdges(i);
      } catch {
        continue;
      }
      const edge = edges.find((e) => e.idHash === el.idHash);
      if (!edge) continue;
      // edge.points are already world-space (kernel.ts), so consume them raw —
      // folding through the placement again would double-transform.
      const pts: [number, number, number][] = [];
      for (let p = 0; p + 2 < edge.points.length; p += 3) {
        pts.push([edge.points[p], edge.points[p + 1], edge.points[p + 2]]);
      }
      if (pts.length < 2) continue;
      let length = 0;
      for (let j = 0; j + 1 < pts.length; j++) {
        length += Math.hypot(
          pts[j + 1][0] - pts[j][0],
          pts[j + 1][1] - pts[j][1],
          pts[j + 1][2] - pts[j][2],
        );
      }
      return { kind: "edge", length, start: pts[0], end: pts[pts.length - 1] };
    }
    return null;
  }

  // vertex
  for (let i = 0; i < count; i++) {
    let verts: BRepVertex[];
    let placement: Placement;
    try {
      verts = k.bodyBrepVertices(i);
      placement = k.bodyPlacement(i);
    } catch {
      continue;
    }
    const v = verts.find((vx) => vx.idHash === el.idHash);
    if (!v) continue;
    return { kind: "vertex", position: localToWorld(v.positionLocal, placement) };
  }
  return null;
}

export function ElementInfo({ store, state }: { store: KernelStore; state: StoreState }) {
  const el = state.selectedElement;
  // Re-resolve when the picked element changes or bodies change (features
  // array reference flips on every commit).
  const resolved = useMemo<Resolved>(
    () => (el ? resolveElement(store, el) : null),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [store, el, state.features],
  );

  if (!el) return null;

  const query = elementQueryString(el);

  return (
    <section className="fd-element-info" data-testid="element-info">
      <div className="header">
        <span className="title" data-testid="element-info-kind">
          {KIND_LABEL[el.kind]}
        </span>
        <button
          type="button"
          className="close"
          title="Auswahl aufheben"
          aria-label="Auswahl aufheben"
          data-testid="element-info-clear"
          onClick={() => store.clearElementSelection()}
        >
          ✕
        </button>
      </div>
      <dl className="body">
        {resolved?.kind === "face" && (
          <>
            <div className="row">
              <dt>Typ</dt>
              <dd>{resolved.surface}</dd>
            </div>
            <div className="row">
              <dt>Schwerpunkt</dt>
              <dd><Vec v={resolved.centroid} /> mm</dd>
            </div>
            <div className="row">
              <dt>Normale</dt>
              <dd><Vec v={resolved.normal} /></dd>
            </div>
          </>
        )}
        {resolved?.kind === "edge" && (
          <>
            <div className="row">
              <dt>Länge</dt>
              <dd>{fmt(resolved.length)} mm</dd>
            </div>
            <div className="row">
              <dt>Start</dt>
              <dd><Vec v={resolved.start} /> mm</dd>
            </div>
            <div className="row">
              <dt>Ende</dt>
              <dd><Vec v={resolved.end} /> mm</dd>
            </div>
          </>
        )}
        {resolved?.kind === "vertex" && (
          <div className="row">
            <dt>Position</dt>
            <dd><Vec v={resolved.position} /> mm</dd>
          </div>
        )}
        {!resolved && (
          <div className="row">
            <dd className="unresolved">Element nicht mehr auflösbar.</dd>
          </div>
        )}
        <div className="row">
          <dt>Query</dt>
          <dd><code className="query" data-testid="element-info-query">{query}</code></dd>
        </div>
      </dl>
      <ElementActions
        store={store}
        el={el}
        faces={state.selectedElements.filter((e) => e.kind === "face")}
      />
    </section>
  );
}

/**
 * Chunk 3b: Onshape-style context-actions. Each button starts the relevant
 * feature directly from the selected element via the SAME store method the
 * viewport-click tool ends up calling, fed the canonical `qFace`/`qEdge`
 * query — no re-pick in the viewport. After firing we clear the selection so
 * the panel doesn't linger on an element a fresh feature may have superseded.
 *
 * Vertices have no actions in this chunk → the section renders nothing.
 */
function ElementActions({
  store,
  el,
  faces,
}: {
  store: KernelStore;
  el: PickedElement;
  faces: PickedElement[];
}) {
  if (el.kind === "vertex") return null;

  const fire = (run: () => void) => {
    run();
    store.clearElementSelection();
  };
  const extrudeLabel =
    faces.length > 1 ? `${faces.length} Flächen extrudieren` : "Fläche extrudieren";
  const thickenLabel =
    faces.length > 1 ? `${faces.length} Flächen verdicken` : "Fläche verdicken";

  return (
    <div className="actions" data-testid="element-info-actions">
      <span className="actions-title">Aktionen</span>
      <div className="action-buttons">
        {el.kind === "face" && (
          <>
            <button
              type="button"
              className="action"
              data-testid="element-action-sketch"
              onClick={() => fire(() => store.enterSketch(elementQueryString(el)))}
            >
              Skizze auf Fläche
            </button>
            <button
              type="button"
              className="action"
              data-testid="element-action-mate"
              onClick={() => fire(() => store.addMateConnectorAt(elementQueryString(el)))}
            >
              Mate-Connector
            </button>
            <button
              type="button"
              className="action"
              data-testid="element-action-extrude"
              onClick={() =>
                fire(() =>
                  store.addExtrudeFaces(
                    (faces.length > 0 ? faces : [el]).map(elementQueryString),
                  ),
                )
              }
            >
              {extrudeLabel}
            </button>
            <button
              type="button"
              className="action"
              data-testid="element-action-thicken"
              onClick={() =>
                fire(() =>
                  store.addThicken(
                    (faces.length > 0 ? faces : [el]).map(elementQueryString),
                  ),
                )
              }
            >
              {thickenLabel}
            </button>
          </>
        )}
        {el.kind === "edge" && (
          <>
            <button
              type="button"
              className="action"
              data-testid="element-action-fillet"
              onClick={() => fire(() => store.addFilletAtEdge(elementQueryString(el)))}
            >
              Verrundung
            </button>
            <button
              type="button"
              className="action"
              data-testid="element-action-chamfer"
              onClick={() => fire(() => store.addChamferAtEdge(elementQueryString(el)))}
            >
              Fase
            </button>
          </>
        )}
      </div>
    </div>
  );
}
