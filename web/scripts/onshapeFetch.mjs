#!/usr/bin/env node
/**
 * Onshape import spike — signed fetch harness (docs/ONSHAPE-IMPORT-PLAN.md).
 *
 * Pulls the feature tree of a Part Studio via the documented REST API, finds
 * the sketch features, and runs the pure translator (../src/onshape/
 * onshapeSketch.ts) to produce FieldDraft sketch sub-calls. Node-only (no CORS
 * in Node), so it needs no backend.
 *
 * Auth: Onshape API key (access + secret), HMAC-SHA256 request signing — the
 * documented, ToS-clean path for reading YOUR OWN documents. No session-token
 * sniffing. Credentials live in a gitignored config, never in the repo.
 *
 * Setup:
 *   1. cp web/.onshape.local.example.json web/.onshape.local.json
 *   2. Fill in accessKey / secretKey (Onshape → Dev Portal → API keys) and the
 *      documentId / workspaceId / elementId from a Part Studio URL:
 *      cad.onshape.com/documents/{doc}/w/{workspace}/e/{element}
 *   3. node web/scripts/onshapeFetch.mjs
 *      (or override the config path: node web/scripts/onshapeFetch.mjs path.json)
 *
 * Output: the raw features JSON (→ /tmp for inspection) plus the translated
 * sub-calls and a diagnostics report of anything the translator didn't map.
 */

import { createHmac, randomBytes } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { register } from "node:module";

const __dirname = dirname(fileURLToPath(import.meta.url));

// The translator is TypeScript; load it through a TS loader if available,
// else fall back to a lightweight inline require via tsx/esbuild. Simplest
// robust path: import the compiled JS if present, else use a dynamic import
// with a .ts specifier under `node --experimental-strip-types` (Node 22.6+).
async function loadTranslator() {
  try {
    const mod = await import("../src/onshape/onshapeSketch.ts");
    return mod.translateOnshapeSketch;
  } catch (err) {
    console.error(
      "Could not import the TS translator directly. Run with Node 22.6+ " +
        "(`node --experimental-strip-types`) or via `npx tsx`.\n" +
        String(err),
    );
    process.exit(1);
  }
}

function loadConfig() {
  const path = process.argv[2]
    ? resolve(process.argv[2])
    : join(__dirname, "..", ".onshape.local.json");
  let raw;
  try {
    raw = readFileSync(path, "utf8");
  } catch {
    console.error(
      `No config at ${path}. Copy web/.onshape.local.example.json to ` +
        `web/.onshape.local.json and fill in your API key + document ids.`,
    );
    process.exit(1);
  }
  const cfg = JSON.parse(raw);
  for (const k of ["accessKey", "secretKey", "documentId", "workspaceId", "elementId"]) {
    if (!cfg[k]) {
      console.error(`Config is missing "${k}".`);
      process.exit(1);
    }
  }
  cfg.baseUrl = cfg.baseUrl || "https://cad.onshape.com";
  return cfg;
}

/**
 * Onshape HMAC request signing (docs: API keys → "Making signed requests").
 * The signature covers method + a nonce + an RFC-1123 date + content-type +
 * the lowercased path & query. Header set: Date, On-Nonce, Authorization.
 */
function signedHeaders(cfg, method, path, query = "") {
  const nonce = randomBytes(25).toString("hex");
  const authDate = new Date().toUTCString();
  const contentType = "application/json";
  const toSign =
    `${method.toLowerCase()}\n${nonce}\n${authDate}\n${contentType}\n` +
    `${path.toLowerCase()}\n${query.toLowerCase()}\n`;
  const hmac = createHmac("sha256", cfg.secretKey)
    .update(toSign)
    .digest("base64");
  return {
    Date: authDate,
    "On-Nonce": nonce,
    "Content-Type": contentType,
    Accept: "application/json;charset=UTF-8; qs=0.09",
    Authorization: `On ${cfg.accessKey}:HmacSHA256:${hmac}`,
  };
}

async function getFeatures(cfg) {
  const path =
    `/api/v9/partstudios/d/${cfg.documentId}` +
    `/w/${cfg.workspaceId}/e/${cfg.elementId}/features`;
  const headers = signedHeaders(cfg, "GET", path);
  const res = await fetch(cfg.baseUrl + path, { method: "GET", headers });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`features request failed: ${res.status} ${body.slice(0, 400)}`);
  }
  return res.json();
}

/** Pull the sketch features out of the features response. The endpoint nests
 *  them under `features[]` with btType `BTMSketch-151` / featureType
 *  `newSketch`; the solved geometry rides on each sketch. */
function findSketches(featuresResponse) {
  const list = featuresResponse?.features ?? [];
  return list.filter(
    (f) =>
      (f?.btType || "").startsWith("BTMSketch") ||
      f?.featureType === "newSketch",
  );
}

async function main() {
  const cfg = loadConfig();
  const translate = await loadTranslator();

  console.log("Fetching features…");
  const features = await getFeatures(cfg);
  const dump = "/tmp/onshape-features.json";
  writeFileSync(dump, JSON.stringify(features, null, 2));
  console.log(`Raw features JSON → ${dump}`);

  const sketches = findSketches(features);
  console.log(`Found ${sketches.length} sketch feature(s).`);

  for (const sketch of sketches) {
    const { subCalls, diagnostics } = translate(sketch);
    console.log(`\n── sketch "${sketch.name ?? sketch.featureId}" ──`);
    console.log(`  ${subCalls.length} sub-call(s):`);
    for (const c of subCalls) console.log("   ", c.type, JSON.stringify(c.params));
    const unhandled = Object.entries(diagnostics.unhandled);
    if (unhandled.length) {
      console.log("  UNHANDLED (to add next):");
      for (const [k, n] of unhandled) console.log(`    ${k} ×${n}`);
    }
    if (diagnostics.notes.length) {
      console.log("  notes:");
      for (const note of diagnostics.notes) console.log(`    ${note}`);
    }
  }
}

// Silence the "register" import lint if a TS loader isn't wired; it's a no-op
// placeholder for future --loader integration.
void register;

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
