import { useMemo } from "react";
import { useKernelStore } from "./store";
import type { PickedElement } from "./elementQuery";
import { formatLength } from "./lengthUnits";
import { useWorkspaceUnits, decimalPlaces } from "./settings";
import { resolveMeasureGeom } from "./measureResolve";
import {
  measureSingle,
  measureDistance,
  type MeasureGeom,
  type Vec3,
} from "./measure";

/**
 * Floating Measure dialog (tickets stv8 + me9w). Shown the whole time Measure
 * mode is active. Each viewport click on a B-Rep VERTEX, EDGE or FACE adds it to
 * `measureSelection`; this panel resolves the picked elements to world geometry
 * (via `resolveMeasureGeom`) and reports, Onshape-style:
 *   - one entity → its intrinsic measure (vertex coords / edge length-or-radius
 *     / face area + centroid);
 *   - two entities → the minimum DISTANCE between them, for ALL nine kind
 *     combinations (point/edge/face × point/edge/face), with X/Y/Z components
 *     and the circle-axis distance between two circular rims.
 */
export function MeasurePanel() {
  const { store, state } = useKernelStore();
  const units = useWorkspaceUnits();
  const lengthUnit = units.lengthUnit;
  const decimals = decimalPlaces(units.lengthDecimals);

  // Resolve every picked element to its world geometry. Re-runs when the
  // selection or the geometry (features) changes — an edit can move an entity.
  const resolved = useMemo<Array<{ el: PickedElement; geom: MeasureGeom | null }>>(
    () => {
      const sel = state?.measureSelection ?? [];
      if (!store || sel.length === 0) return [];
      return sel.map((el) => ({
        el,
        geom: (() => {
          try {
            return resolveMeasureGeom(store.kernel, el);
          } catch {
            return null;
          }
        })(),
      }));
      // eslint-disable-next-line react-hooks/exhaustive-deps
    },
    [store, state?.measureSelection, state?.features],
  );

  if (!state?.measureMode) return null;

  const fmt = (mm: number) => formatLength(mm, lengthUnit, decimals);

  return (
    <div className="fd-measure-panel" data-testid="measure-panel">
      <div className="header">
        <span className="title">Measure</span>
        <button
          className="close"
          title="Measure schließen"
          onClick={() => store?.toggleMeasureMode()}
        >
          ×
        </button>
      </div>

      <div className="section-label">Select entities to measure</div>
      {resolved.length === 0 ? (
        <div className="hint" data-testid="measure-empty">
          Klicke auf eine B-Rep-Ecke, -Kante oder -Fläche.
        </div>
      ) : (
        <ul className="entities">
          {resolved.map((r, i) => (
            <li key={`${r.el.kind}:${r.el.idHash}`} className="entity">
              <span className="entity-name">{entityLabel(r.el, r.geom, i)}</span>
              <button
                className="remove"
                title="Entfernen"
                onClick={() => store?.removeMeasureEntity(r.el)}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}

      {renderReadout(resolved, fmt)}
    </div>
  );
}

const KIND_LABEL: Record<PickedElement["kind"], string> = {
  vertex: "Ecke",
  edge: "Kante",
  face: "Fläche",
};

function entityLabel(
  el: PickedElement,
  geom: MeasureGeom | null,
  i: number,
): string {
  const base = KIND_LABEL[el.kind];
  if (!geom) return `${base} ${i + 1} (fehlt)`;
  return `${base} ${i + 1}`;
}

function fmtVec(v: Vec3): string {
  return v.map((c) => c.toFixed(2)).join(", ");
}

function renderReadout(
  resolved: Array<{ el: PickedElement; geom: MeasureGeom | null }>,
  fmt: (mm: number) => string,
) {
  const have = resolved.filter((r) => r.geom) as Array<{
    el: PickedElement;
    geom: MeasureGeom;
  }>;
  if (have.length === 0) return null;

  // ── single entity ────────────────────────────────────────────
  if (have.length === 1) {
    return (
      <div className="readout" data-testid="measure-readout">
        {renderSingle(have[0].geom, fmt)}
      </div>
    );
  }

  // ── two entities → distance (all combinations) ───────────────
  const a = have[0].geom;
  const b = have[1].geom;
  const dist = measureDistance(a, b);

  // Axis distance between two circular rims (both edges + both circular).
  let centerDist: number | null = null;
  if (a.kind === "edge" && b.kind === "edge") {
    const ea = measureSingle(a);
    const eb = measureSingle(b);
    if (ea.circle && eb.circle) {
      const c0 = ea.circle.center;
      const c1 = eb.circle.center;
      centerDist = Math.hypot(c1[0] - c0[0], c1[1] - c0[1], c1[2] - c0[2]);
    }
  }

  return (
    <div className="readout" data-testid="measure-readout">
      {dist ? (
        <>
          <Row label="Min-Distanz" value={fmt(dist.min)} />
          <Row label="ΔX" value={fmt(Math.abs(dist.delta[0]))} indent />
          <Row label="ΔY" value={fmt(Math.abs(dist.delta[1]))} indent />
          <Row label="ΔZ" value={fmt(Math.abs(dist.delta[2]))} indent />
        </>
      ) : (
        <div className="row">
          <span className="row-label">Keine Distanz</span>
        </div>
      )}
      {centerDist !== null ? (
        <Row label="Achsabstand (Mittelpunkte)" value={fmt(centerDist)} />
      ) : null}
    </div>
  );
}

/** Intrinsic readout for one resolved entity. */
function renderSingle(g: MeasureGeom, fmt: (mm: number) => string) {
  const m = measureSingle(g);
  if (m.kind === "vertex") {
    return <Row label="Position" value={`${fmtVec(m.position!)} mm`} />;
  }
  if (m.kind === "edge") {
    if (m.circle) {
      return (
        <>
          <Row label="Radius" value={fmt(m.circle.radius)} />
          <Row label="Durchmesser" value={fmt(m.circle.radius * 2)} />
          <Row label="Umfang" value={fmt(m.length ?? 0)} />
          <Row label="Mittelpunkt" value={fmtVec(m.circle.center)} />
        </>
      );
    }
    return <Row label="Länge" value={fmt(m.length ?? 0)} />;
  }
  // face
  return (
    <>
      <Row label="Typ" value={m.surfaceLabel ?? "Fläche"} />
      <Row label="Flächeninhalt" value={`${(m.area ?? 0).toFixed(2)} mm²`} />
      <Row label="Schwerpunkt" value={fmtVec(m.centroid ?? [0, 0, 0])} />
    </>
  );
}

function Row({
  label,
  value,
  indent,
}: {
  label: string;
  value: string;
  indent?: boolean;
}) {
  return (
    <div className={indent ? "row indent" : "row"}>
      <span className="row-label">{label}</span>
      <span className="row-value">{value}</span>
    </div>
  );
}
