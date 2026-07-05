/**
 * Debug ▸ Part-AABB readout.
 *
 * A compact floating panel that lists the numeric bounds of every selected
 * part while the Debug ▸ Part-AABB overlay is on: size (X × Y × Z) and the
 * min/max corner, in mm. Mirrors the box(es) `PartAabbOverlay` draws. Hidden
 * when the toggle is off or nothing is selected.
 */

import { useMemo, useSyncExternalStore } from "react";
import { useKernelStore } from "./store";
import { getPartAabbVisible, subscribePartAabb } from "./partAabbStore";
import { selectedPartAabbs, type PartAabb } from "./partAabb";

const f2 = (n: number) => n.toFixed(2);
const vec = (v: readonly [number, number, number]) =>
  `${f2(v[0])}, ${f2(v[1])}, ${f2(v[2])}`;

export function PartAabbPanel() {
  const { store, state } = useKernelStore();
  const visible = useSyncExternalStore(subscribePartAabb, getPartAabbVisible);
  const selection = state?.selection;

  // Re-resolve when the selection OR the geometry (an edit moves/cuts a part)
  // changes — `features`/`brepVersion` are extra invalidation triggers.
  const boxes = useMemo<PartAabb[]>(() => {
    if (!visible || !store || !selection || selection.size === 0) return [];
    return selectedPartAabbs(store.kernel, selection);
  }, [visible, store, selection, state?.features, state?.brepVersion]);

  if (!visible) return null;

  return (
    <div className="fd-part-aabb-panel" data-testid="part-aabb-panel">
      <div className="title">Part-AABB</div>
      {boxes.length === 0 ? (
        <div className="hint">Kein Part selektiert.</div>
      ) : (
        <ul className="parts">
          {boxes.map((b, i) => (
            <li className="part" key={i}>
              {boxes.length > 1 && <div className="part-name">Part {i + 1}</div>}
              <div className="row">
                <span className="k">Größe</span>
                <span className="v">
                  {f2(b.size[0])} × {f2(b.size[1])} × {f2(b.size[2])} mm
                </span>
              </div>
              <div className="row">
                <span className="k">Min</span>
                <span className="v">{vec(b.min)}</span>
              </div>
              <div className="row">
                <span className="k">Max</span>
                <span className="v">{vec(b.max)}</span>
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
