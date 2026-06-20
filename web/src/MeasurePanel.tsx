import { useMemo } from "react";
import { useKernelStore } from "./store";
import type { PickedElement } from "./elementQuery";
import { formatLength } from "./lengthUnits";
import { useWorkspaceUnits, decimalPlaces } from "./SettingsMenu";
import {
  analyzeEdge,
  minDistanceBetweenPolylines,
  type EdgeMeasure,
} from "./measure";

/**
 * Floating Measure dialog (ticket stv8). Shown the whole time Measure mode is
 * active. Each viewport click on a B-Rep edge adds it to `measureSelection`;
 * this panel resolves the picked edges to their world polylines (via
 * `kernel.bodyBrepEdges`) and reports, Onshape-style:
 *   - one edge → Length, or Radius/Diameter/Centre for a circular rim;
 *   - two edges → minimum Distance with X/Y/Z components (+ axis distance
 *     between two circle centres).
 */
export function MeasurePanel() {
  const { store, state } = useKernelStore();
  const units = useWorkspaceUnits();
  const lengthUnit = units.lengthUnit;
  const decimals = decimalPlaces(units.lengthDecimals);

  // Resolve every picked element to its world-space polyline. Re-runs when the
  // selection or the geometry (features) changes — an edit can move an edge.
  const resolved = useMemo<Array<{ el: PickedElement; points: number[] | null }>>(() => {
    const sel = state?.measureSelection ?? [];
    if (!store || sel.length === 0) return [];
    // One scan of all bodies' edges → id_hash → polyline.
    const byHash = new Map<string, number[]>();
    try {
      const n = store.kernel.bodyCount();
      for (let i = 0; i < n; i++) {
        let edges;
        try {
          edges = store.kernel.bodyBrepEdges(i);
        } catch {
          continue;
        }
        for (const e of edges) {
          if (!byHash.has(e.idHash)) byHash.set(e.idHash, e.points);
        }
      }
    } catch {
      /* kernel not ready */
    }
    return sel.map((el) => ({ el, points: byHash.get(el.idHash) ?? null }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, state?.measureSelection, state?.features]);

  if (!state?.measureMode) return null;

  const fmt = (mm: number) => formatLength(mm, lengthUnit, decimals);
  const edgeMeasures: EdgeMeasure[] = resolved.map((r) =>
    r.points ? (analyzeEdge(r.points) ?? emptyEdge()) : emptyEdge(),
  );

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
          Klicke auf eine B-Rep-Kante (z.&nbsp;B. den Rand des Revolve).
        </div>
      ) : (
        <ul className="entities">
          {resolved.map((r, i) => (
            <li key={`${r.el.kind}:${r.el.idHash}`} className="entity">
              <span className="entity-name">
                {r.points ? `Kante ${i + 1}` : `Kante ${i + 1} (fehlt)`}
              </span>
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

      {renderReadout(resolved, edgeMeasures, fmt)}
    </div>
  );
}

function emptyEdge(): EdgeMeasure {
  return { length: 0, closed: false, circle: null };
}

function renderReadout(
  resolved: Array<{ el: PickedElement; points: number[] | null }>,
  measures: EdgeMeasure[],
  fmt: (mm: number) => string,
) {
  const have = resolved.filter((r) => r.points);
  if (have.length === 0) return null;

  // ── single edge ──────────────────────────────────────────────
  if (have.length === 1) {
    const m = measures[resolved.indexOf(have[0])];
    return (
      <div className="readout" data-testid="measure-readout">
        {m.circle ? (
          <>
            <Row label="Radius" value={fmt(m.circle.radius)} />
            <Row label="Durchmesser" value={fmt(m.circle.radius * 2)} />
            <Row label="Umfang" value={fmt(m.length)} />
            <Row
              label="Mittelpunkt"
              value={m.circle.center.map((c) => c.toFixed(2)).join(", ")}
            />
          </>
        ) : (
          <Row label="Länge" value={fmt(m.length)} />
        )}
      </div>
    );
  }

  // ── two edges → distance ─────────────────────────────────────
  const a = have[0];
  const b = have[1];
  const dist = minDistanceBetweenPolylines(a.points!, b.points!);
  const ma = measures[resolved.indexOf(a)];
  const mb = measures[resolved.indexOf(b)];
  let centerDist: number | null = null;
  if (ma.circle && mb.circle) {
    const c0 = ma.circle.center;
    const c1 = mb.circle.center;
    centerDist = Math.hypot(c1[0] - c0[0], c1[1] - c0[1], c1[2] - c0[2]);
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
      ) : null}
      {centerDist !== null ? (
        <Row label="Achsabstand (Mittelpunkte)" value={fmt(centerDist)} />
      ) : null}
    </div>
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
