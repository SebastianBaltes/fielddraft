/// <reference types="vite/client" />

declare module "*.css";

/** Vite `?worker` imports resolve to a Worker constructor. Used by the
 *  lazy Monaco editor module to wire up `self.MonacoEnvironment`. */
declare module "*?worker" {
  const w: new () => Worker;
  export default w;
}

/** Vite `?raw` imports resolve to the file's text content. Used to feed
 *  `fielddraft.d.ts` into Monaco's TypeScript worker as an extraLib. */
declare module "*?raw" {
  const s: string;
  export default s;
}

declare const __APP_VERSION__: string;
