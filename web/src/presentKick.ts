/**
 * Cross-mode WebGPU "present kick" signal.
 *
 * On Linux/NVIDIA the first WebGPU swapchain comes up in a state where frames
 * render and submit but the compositor never presents them — the canvas stays
 * blank until the surface is reconfigured *with an actual pixel-size delta*.
 * Confirmed empirically: the renderer, canvas backing buffer and container all
 * agree on size, yet nothing shows until the window is resized. A same-size
 * reconfigure (re-`configure()`-ing the GPUCanvasContext at identical
 * dimensions) is NOT enough — only a real dimension change unsticks it.
 *
 * `PresentKick` (see viewportOverlays) consumes this signal and performs a
 * one-frame ±1px backing-buffer jiggle, mirroring the manual window resize.
 * The signal is mode-independent: it covers Mesh mode (no shader compile) and
 * the SDF blank-during-compile window alike.
 */
let pending = false;

/** Request a present kick on the next rendered frame. */
export function requestPresentKick(): void {
  pending = true;
}

/** Consume a pending kick request (returns true at most once per request). */
export function consumePresentKick(): boolean {
  if (!pending) return false;
  pending = false;
  return true;
}
