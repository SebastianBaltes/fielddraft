/**
 * Debug ▸ Voxel dialog (#369).
 *
 * Fills the scene solid with voxels at a chosen resolution and lets you diff
 * two snapshots to quantify a geometry change (e.g. with vs. without an edge
 * fillet, #373): save a snapshot → change the geometry → save another → show
 * the difference as GREEN (added) / RED (removed) voxels plus a quantitative
 * readout (added / removed / unchanged counts and the volume delta).
 *
 * The solid voxel fill is shown for as long as THIS DIALOG stays open (#379,
 * like Debug ▸ Mesher / Debug ▸ Lichen scope their overlays to their own
 * dialog) — it is not tied to a render mode. The diff overlay is drawn on top
 * in ANY render mode. Non-modal + draggable, like the Mesher dialog. Console
 * equivalent: `window.__fdVoxel`.
 */

import {
  useCallback,
  useEffect,
  useState,
  useSyncExternalStore,
} from "react";
import { useKernelStore } from "./store";
import { NumberInput } from "./NumberInput";
import { useDialogDrag } from "./useDialogDrag";
import {
  MAX_VOXEL_RESOLUTION,
  MIN_VOXEL_RESOLUTION,
  getVoxelDisplay,
  getVoxelResolution,
  setVoxelDiff,
  setVoxelResolution,
  setVoxelShowUnchanged,
  setVoxelSolidActive,
  subscribeVoxelDisplay,
} from "./voxelDebugStore";
import { diffSnapshots, type VoxelSnapshot } from "./voxelSnapshot";

interface StoredSnap {
  id: number;
  label: string;
  snap: VoxelSnapshot;
}

interface DiffReadout {
  added: number;
  removed: number;
  unchanged: number;
  volumeDelta: number;
}

let nextSnapId = 1;

export function VoxelDialog({ onClose }: { onClose: () => void }) {
  const { store } = useKernelStore();
  const display = useSyncExternalStore(subscribeVoxelDisplay, getVoxelDisplay);

  const [resolution, setResolution] = useState(getVoxelResolution());
  const [snaps, setSnaps] = useState<StoredSnap[]>([]);
  const [refId, setRefId] = useState<number | null>(null);
  const [cmpId, setCmpId] = useState<number | "current">("current");
  const [readout, setReadout] = useState<DiffReadout | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  // Publish the resolution to the store (drives the solid fill + the console
  // API default) whenever it changes.
  useEffect(() => {
    setVoxelResolution(resolution);
  }, [resolution]);

  // The solid voxel fill is active for exactly as long as this dialog is
  // open (#379) — mirrors Debug ▸ Mesher / Debug ▸ Lichen. Clear the diff
  // overlay on close too: it is a transient measurement, not persistent
  // scene state.
  useEffect(() => {
    setVoxelSolidActive(true);
    return () => {
      setVoxelSolidActive(false);
      setVoxelDiff(null);
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        e.stopPropagation();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const saveSnapshot = useCallback(() => {
    if (!store) return;
    const snap = store.voxelize(resolution);
    if (!snap) {
      setMessage("Szene ist leer — nichts zu voxelisieren.");
      return;
    }
    const id = nextSnapId++;
    const label = `#${id} · ${snap.dims.join("×")} · ${snap.count.toLocaleString()} Voxel`;
    setSnaps((prev) => [...prev, { id, label, snap }]);
    // First save → reference; later saves → compare target. This makes the
    // "before → change → after" workflow one click each.
    setRefId((prev) => (prev === null ? id : prev));
    setCmpId(id);
    setMessage(null);
  }, [store, resolution]);

  const showDiff = useCallback(() => {
    if (!store) return;
    const a = snaps.find((s) => s.id === refId)?.snap;
    if (!a) {
      setMessage("Kein Referenz-Snapshot gewählt.");
      return;
    }
    const b =
      cmpId === "current"
        ? store.voxelize(resolution)
        : snaps.find((s) => s.id === cmpId)?.snap;
    if (!b) {
      setMessage("Kein Vergleichs-Snapshot verfügbar.");
      return;
    }
    const res = diffSnapshots(a, b);
    if ("error" in res) {
      setVoxelDiff(null);
      setReadout(null);
      setMessage(res.error);
      return;
    }
    // Unchanged mask for the optional faint-grey rendering (both occupied).
    const unchangedMask = new Uint8Array(b.occupancy.length);
    for (let i = 0; i < unchangedMask.length; i++) {
      if (a.occupancy[i] && b.occupancy[i]) unchangedMask[i] = 1;
    }
    setVoxelDiff({
      dims: b.dims,
      origin: b.origin,
      spacing: b.spacing,
      addedMask: res.addedMask,
      removedMask: res.removedMask,
      unchangedMask,
      added: res.added,
      removed: res.removed,
    });
    setReadout({
      added: res.added,
      removed: res.removed,
      unchanged: res.unchanged,
      volumeDelta: res.volumeDelta,
    });
    setMessage(null);
  }, [store, snaps, refId, cmpId, resolution]);

  const clearDiff = useCallback(() => {
    setVoxelDiff(null);
    setReadout(null);
    setMessage(null);
  }, []);

  const removeSnap = useCallback((id: number) => {
    setSnaps((prev) => prev.filter((s) => s.id !== id));
    setRefId((prev) => (prev === id ? null : prev));
    setCmpId((prev) => (prev === id ? "current" : prev));
  }, []);

  // ── Drag the header (mirrors MesherDialog) ───────────────────────
  const { positionStyle, onHeaderDown, onHeaderMove, onHeaderUp } = useDialogDrag();

  const fmtVol = (v: number) =>
    `${v >= 0 ? "+" : ""}${v.toLocaleString(undefined, { maximumFractionDigits: 2 })} mm³`;

  return (
    <div
      className="fd-voxel-dialog"
      role="dialog"
      aria-label="Voxel"
      data-testid="voxel-dialog"
      style={positionStyle}
    >
      <div
        className="dialog-header"
        onPointerDown={onHeaderDown}
        onPointerMove={onHeaderMove}
        onPointerUp={onHeaderUp}
      >
        <h2 className="dialog-title">Voxel — Vergleich</h2>
        <button
          type="button"
          className="dialog-close"
          aria-label="Close"
          onClick={onClose}
        >
          ×
        </button>
      </div>

      <div className="vec-row">
        <label title="Anzahl Voxel entlang der längsten Achse der Szenen-AABB. Die anderen Achsen bekommen proportional viele, damit die Voxel würfelförmig bleiben.">
          Auflösung
        </label>
        <div className="fields">
          <NumberInput
            min={MIN_VOXEL_RESOLUTION}
            max={MAX_VOXEL_RESOLUTION}
            step={1}
            integer
            aria-label="Auflösung"
            value={resolution}
            onChange={setResolution}
            commitOnChange={false}
            data-testid="voxel-resolution"
          />
        </div>
      </div>

      <p className="hint">
        Solange dieser Dialog offen ist, wird die Szene voxelisiert dargestellt.
        Speichere einen Snapshot, ändere die Geometrie, speichere erneut und
        zeige den Unterschied.
      </p>

      <div className="btn-row">
        <button
          type="button"
          className="voxel-btn primary"
          data-testid="voxel-save-snapshot"
          onClick={saveSnapshot}
        >
          Snapshot speichern
        </button>
      </div>

      {snaps.length > 0 && (
        <ul className="snap-list" data-testid="voxel-snap-list">
          {snaps.map((s) => (
            <li key={s.id} className="snap-row">
              <span className="snap-label">{s.label}</span>
              <button
                type="button"
                className="voxel-btn tiny"
                aria-label={`Snapshot ${s.id} löschen`}
                onClick={() => removeSnap(s.id)}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}

      <div className="vec-row">
        <label title="Referenz A (vorher).">Referenz (A)</label>
        <div className="fields">
          <select
            className="voxel-select"
            data-testid="voxel-ref-select"
            value={refId ?? ""}
            onChange={(e) =>
              setRefId(e.target.value === "" ? null : Number(e.target.value))
            }
          >
            <option value="">— wählen —</option>
            {snaps.map((s) => (
              <option key={s.id} value={s.id}>
                {s.label}
              </option>
            ))}
          </select>
        </div>
      </div>

      <div className="vec-row">
        <label title="Vergleich B (nachher).">Vergleich (B)</label>
        <div className="fields">
          <select
            className="voxel-select"
            data-testid="voxel-cmp-select"
            value={cmpId}
            onChange={(e) =>
              setCmpId(
                e.target.value === "current"
                  ? "current"
                  : Number(e.target.value),
              )
            }
          >
            <option value="current">Aktuelle Geometrie</option>
            {snaps.map((s) => (
              <option key={s.id} value={s.id}>
                {s.label}
              </option>
            ))}
          </select>
        </div>
      </div>

      <div className="btn-row">
        <button
          type="button"
          className="voxel-btn primary"
          data-testid="voxel-show-diff"
          onClick={showDiff}
        >
          Diff anzeigen
        </button>
        <button
          type="button"
          className="voxel-btn"
          data-testid="voxel-clear-diff"
          onClick={clearDiff}
        >
          Ausblenden
        </button>
      </div>

      <div className="vec-row check-row">
        <label htmlFor="voxel-show-unchanged">Unveränderte zeigen</label>
        <input
          id="voxel-show-unchanged"
          type="checkbox"
          data-testid="voxel-show-unchanged"
          checked={display.showUnchanged}
          onChange={(e) => setVoxelShowUnchanged(e.target.checked)}
        />
      </div>

      {message && (
        <p className="voxel-message" data-testid="voxel-message">
          {message}
        </p>
      )}

      {readout && (
        <div className="voxel-readout" data-testid="voxel-readout">
          <div className="readout">
            <span className="k added">Hinzugekommen (grün)</span>
            <span className="v" data-testid="voxel-added">
              {readout.added.toLocaleString()}
            </span>
          </div>
          <div className="readout">
            <span className="k removed">Entfernt (rot)</span>
            <span className="v" data-testid="voxel-removed">
              {readout.removed.toLocaleString()}
            </span>
          </div>
          <div className="readout">
            <span className="k">Unverändert</span>
            <span className="v">{readout.unchanged.toLocaleString()}</span>
          </div>
          <div className="readout">
            <span className="k">Volumen-Delta</span>
            <span className="v" data-testid="voxel-volume-delta">
              {fmtVol(readout.volumeDelta)}
            </span>
          </div>
        </div>
      )}
    </div>
  );
}
