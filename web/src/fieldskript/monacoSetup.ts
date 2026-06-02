/**
 * Lazy Monaco bootstrap for the FieldSkript editor (S4).
 *
 * Monaco is heavy (≈ several hundred KB + language workers) and pulls
 * in `?worker` imports that jsdom can't run, so this whole module is
 * imported dynamically — only when the editor panel actually mounts.
 * Keeping the `MonacoEnvironment` wiring here (rather than in
 * `main.tsx`) guarantees the worker chunks load lazily alongside it.
 *
 * The module:
 *  - registers the editor + TypeScript language workers via Vite's
 *    `?worker` imports,
 *  - configures the TypeScript language service ONCE (idempotent),
 *  - feeds `fielddraft.d.ts` to the TS worker as an extraLib so
 *    `import { fd } from "fielddraft"` resolves with full IntelliSense,
 *  - exposes {@link createScriptEditor}, a thin factory over
 *    `monaco.editor.create` returning a tiny handle.
 */

import * as monaco from "monaco-editor";
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import tsWorker from "monaco-editor/esm/vs/language/typescript/ts.worker?worker";
import fielddraftDts from "./fielddraft.d.ts?raw";

// MonacoEnvironment is a global Monaco reads at runtime to locate its
// language/editor workers.
(self as unknown as { MonacoEnvironment: unknown }).MonacoEnvironment = {
  getWorker(_id: string, label: string): Worker {
    if (label === "typescript" || label === "javascript") return new tsWorker();
    return new editorWorker();
  },
};

/** Guard so the language-service config + extraLib registration only
 *  runs once even if the editor is opened/closed repeatedly. */
let configured = false;

/** Monotonic counter so every editor model gets a unique `.ts` URI —
 *  required for the TS worker to type-check each buffer independently. */
let modelCounter = 0;

function configureTypeScriptOnce(): void {
  if (configured) return;
  configured = true;

  const ts = monaco.typescript.typescriptDefaults;
  ts.setCompilerOptions({
    target: monaco.typescript.ScriptTarget.ESNext,
    moduleResolution: monaco.typescript.ModuleResolutionKind.NodeJs,
    module: monaco.typescript.ModuleKind.ESNext,
    allowNonTsExtensions: true,
    strict: true,
    noEmit: true,
  });

  // `fielddraft.d.ts` already wraps everything in
  // `declare module "fielddraft" { … }`, so registering it under an
  // `@types` path lets `import … from "fielddraft"` resolve to it.
  ts.addExtraLib(
    fielddraftDts,
    "file:///node_modules/@types/fielddraft/index.d.ts",
  );
}

/** A minimal handle over a Monaco editor instance. */
export interface ScriptEditorHandle {
  getValue(): string;
  setValue(value: string): void;
  /** Force a relayout — needed after the host transitions from
   *  display:none back into view (automaticLayout can't measure a
   *  hidden container). */
  layout(): void;
  dispose(): void;
}

/**
 * Create a TypeScript editor inside `container`, seeded with
 * `initialValue`, calling `onChange` on every keystroke. The model
 * uses a unique `.ts` URI so the TS worker emits live diagnostics
 * against the `fielddraft` extraLib.
 */
export function createScriptEditor(
  container: HTMLElement,
  initialValue: string,
  onChange: (value: string) => void,
): ScriptEditorHandle {
  configureTypeScriptOnce();

  modelCounter += 1;
  const uri = monaco.Uri.parse(
    `file:///fieldskript/script_${modelCounter}.ts`,
  );
  const model = monaco.editor.createModel(initialValue, "typescript", uri);

  const editor = monaco.editor.create(container, {
    model,
    theme: "vs",
    fontSize: 16,
    minimap: { enabled: false },
    automaticLayout: true,
    scrollBeyondLastLine: false,
    tabSize: 2,
  });

  const sub = editor.onDidChangeModelContent(() => {
    onChange(editor.getValue());
  });

  return {
    getValue: () => editor.getValue(),
    setValue: (value: string) => editor.setValue(value),
    layout: () => editor.layout(),
    dispose: () => {
      sub.dispose();
      editor.dispose();
      model.dispose();
    },
  };
}
