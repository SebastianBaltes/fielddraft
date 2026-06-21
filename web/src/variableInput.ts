/**
 * Variable input model — the pure, testable core behind the Variables panel's
 * single smart value field (tickets buy9 + nlij).
 *
 * The user types ONE free-text field per variable. It can be:
 *   - a plain number              → `42`          (Number)
 *   - a number with a unit        → `10 cm`       (Length, normalised to mm)
 *   - an angle                    → `90 deg`      (Angle, normalised to °)
 *   - a formula referencing other
 *     variables / math functions  → `sin(Breite) * 2`, `Breite + 5 mm`
 *
 * There is no kind selector and no `ƒx` toggle: the dimension is inferred from
 * the unit (or the formula's resolved dimension), and "formula vs literal" is
 * read straight from the text. Everything here is a deterministic pure function
 * so it can be unit-tested without a browser or the WASM kernel.
 *
 * The kernel stores each variable's *resolved* value in its display unit
 * (mm for Length, ° for Angle, raw for Number) plus the raw source text, so a
 * `=name` feature formula reads a plain number and the panel can re-show what
 * the user typed.
 */

import { LENGTH_TO_MM } from "./lengthUnits";

export type VarKind = "number" | "length" | "angle";

/** Angle units → degrees (the Angle display/base unit). */
const ANGLE_TO_DEG: Record<string, number> = {
  deg: 1,
  "°": 1,
  rad: 180 / Math.PI,
};

/** Functions the formula grammar understands — used to tell a formula apart
 *  from a bare identifier and to validate. Mirrors the kernel's `eval_builtin`
 *  (crates/kernel/src/expr.rs); keep the two in sync. */
const BUILTIN_FUNCS = new Set([
  "sin", "cos", "tan", "asin", "acos", "atan", "atan2",
  "sqrt", "abs", "floor", "ceil", "round", "sign", "ln",
  "pow", "min", "max", "deg2rad", "rad2deg",
]);

/** Bare-identifier constants (resolve when not shadowed by a variable). */
const BUILTIN_CONSTS: Record<string, number> = {
  pi: Math.PI,
  tau: Math.PI * 2,
  e: Math.E,
};

/** The dimension a parsed input commits to. `null` = inherit / undecided. */
export type Dimension = VarKind;

export interface ParseError {
  ok: false;
  error: string;
}

/** A successfully parsed variable input. `value` is in the dimension's base
 *  unit (mm / ° / raw). `formula` is the trimmed source text when the input is
 *  a formula (so it must be re-resolved against other variables), otherwise
 *  the raw literal text (`"10 cm"`) for redisplay — or undefined for a plain
 *  number with no unit. `isFormula` decides whether `value` is final or has to
 *  flow through {@link resolveVariableTable}. */
export interface ParsedInput {
  ok: true;
  /** Resolved literal value in base unit, or NaN for formulas (value comes
   *  from resolution). */
  value: number;
  kind: VarKind;
  isFormula: boolean;
  /** Raw text to persist (formula expression, or `"10 cm"` literal). Undefined
   *  for a unitless plain number — that round-trips through `value` alone. */
  source?: string;
}

export type ParseResult = ParsedInput | ParseError;

const NUMBER_LITERAL = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/;
const NUMBER_UNIT = /^([+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?)\s*([a-zA-Z°]+)$/;

/** Recognise a unit token → its dimension + base-unit factor. Case-insensitive
 *  for length/angle word units; `°` kept verbatim. */
function lookupUnit(
  unit: string,
): { kind: VarKind; factor: number } | null {
  const lower = unit.toLowerCase();
  if (lower in LENGTH_TO_MM) return { kind: "length", factor: LENGTH_TO_MM[lower] };
  if (unit === "°") return { kind: "angle", factor: 1 };
  if (lower in ANGLE_TO_DEG) return { kind: "angle", factor: ANGLE_TO_DEG[lower] };
  return null;
}

/** True when `text` looks like a formula rather than a literal: it contains an
 *  operator, a function call / identifier, or parentheses — anything that isn't
 *  a single number-or-number+unit literal. Used to drive the no-toggle UX. */
export function looksLikeFormula(text: string): boolean {
  const t = text.trim().replace(/^=/, "").trim();
  if (t.length === 0) return false;
  if (NUMBER_LITERAL.test(t)) return false;
  if (NUMBER_UNIT.test(t)) return false;
  return true;
}

/** Tokens that name another variable / constant / function in a formula. Used
 *  both for dependency extraction (cycle detection) and validation. */
function identifiers(expr: string): string[] {
  const out: string[] = [];
  // Match identifiers (Unicode letters via \p{L}); skip the ones immediately
  // followed by `(` (function calls) for the dependency set.
  const re = /[\p{L}_][\p{L}\p{N}_]*/gu;
  let m: RegExpExecArray | null;
  while ((m = re.exec(expr)) !== null) {
    const name = m[0];
    const after = expr.slice(re.lastIndex).trimStart();
    const isCall = after.startsWith("(");
    if (isCall) continue;
    if (name in BUILTIN_CONSTS) continue;
    out.push(name);
  }
  return out;
}

/** Variable names a formula depends on (other variables only — functions and
 *  constants filtered out). Order-preserving, de-duplicated. */
export function formulaDependencies(
  expr: string,
  knownVars: ReadonlySet<string>,
): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const id of identifiers(expr.replace(/^=/, ""))) {
    if (!knownVars.has(id)) continue; // a non-variable ident → resolution error later
    if (seen.has(id)) continue;
    seen.add(id);
    out.push(id);
  }
  return out;
}

/**
 * Parse a single value-field input into its dimension + resolved-or-deferred
 * value. `knownVars` lets a bare identifier (`Breite`) be recognised as a
 * formula rather than rejected as a stray word.
 */
export function parseVariableInput(
  raw: string,
  knownVars: ReadonlySet<string> = new Set(),
): ParseResult {
  const text = raw.trim();
  if (text.length === 0) return { ok: false, error: "empty" };

  // Strip an optional leading `=` (we accept but never require it).
  const body = text.replace(/^=\s*/, "");

  // 1) Plain number → Number.
  if (NUMBER_LITERAL.test(body)) {
    const value = Number(body);
    if (!Number.isFinite(value)) return { ok: false, error: "not a finite number" };
    return { ok: true, value, kind: "number", isFormula: false };
  }

  // 2) Number + unit → Length / Angle, normalised to base unit.
  const unitMatch = body.match(NUMBER_UNIT);
  if (unitMatch) {
    const n = Number(unitMatch[1]);
    const unit = unitMatch[2];
    const u = lookupUnit(unit);
    if (!u) return { ok: false, error: `unknown unit \`${unit}\`` };
    if (!Number.isFinite(n)) return { ok: false, error: "not a finite number" };
    return {
      ok: true,
      value: n * u.factor,
      kind: u.kind,
      isFormula: false,
      source: `${n} ${unit}`,
    };
  }

  // 3) Otherwise it's a formula. Validate identifiers shallowly (a leftover
  //    word that's neither a known variable, a function, nor a constant is a
  //    likely typo we can flag up-front; the real eval happens in the kernel).
  const re = /[\p{L}_][\p{L}\p{N}_]*/gu;
  let m: RegExpExecArray | null;
  while ((m = re.exec(body)) !== null) {
    const name = m[0];
    const after = body.slice(re.lastIndex).trimStart();
    if (after.startsWith("(")) {
      if (!BUILTIN_FUNCS.has(name.toLowerCase()))
        return { ok: false, error: `unknown function \`${name}\`` };
      continue;
    }
    if (name in BUILTIN_CONSTS) continue;
    if (!knownVars.has(name))
      return { ok: false, error: `unknown variable \`${name}\`` };
  }

  // Dimension of a formula is inferred at resolution time (from the result's
  // referenced variables); default to Number here. The caller resolves it.
  return { ok: true, value: NaN, kind: "number", isFormula: true, source: body };
}

// ─── variable-table resolution (cycle-safe) ──────────────────────────────

export interface RawVariable {
  name: string;
  /** Resolved literal value in base unit (for non-formula vars). */
  value: number;
  kind: VarKind;
  /** Formula source text when the variable is a formula. */
  formula?: string;
}

export interface ResolvedVariable {
  name: string;
  value: number;
  kind: VarKind;
  formula?: string;
}

export interface ResolveResult {
  resolved: Map<string, ResolvedVariable>;
  /** Per-variable error message (cycle / unknown ref / bad formula). */
  errors: Map<string, string>;
}

/**
 * Resolve a whole variable table, evaluating formula variables against the
 * others. Detects reference cycles (A=B, B=A) and reports them as a clear
 * per-variable error instead of recursing forever. Literal variables resolve
 * to their stored value; formula variables evaluate their expression with the
 * resolved values of their dependencies.
 *
 * The evaluator here is a small mirror of the kernel grammar so the panel can
 * show a live resolved value without a kernel round-trip; the kernel remains
 * the authority for feature regeneration.
 */
export function resolveVariableTable(vars: readonly RawVariable[]): ResolveResult {
  const byName = new Map<string, RawVariable>();
  for (const v of vars) byName.set(v.name, v);
  const known = new Set(byName.keys());

  const resolved = new Map<string, ResolvedVariable>();
  const errors = new Map<string, string>();
  const inProgress = new Set<string>();

  function resolve(name: string, stack: string[]): number {
    const cached = resolved.get(name);
    if (cached) return cached.value;
    const v = byName.get(name);
    if (!v) throw new FormulaEvalError(`unknown variable \`${name}\``);

    // A variable is only a *formula* (needing evaluation against the others)
    // when its source text reads as an expression. A unit literal like
    // `"10 cm"` also carries source text — but that's for redisplay only; its
    // resolved `value` is authoritative, so it short-circuits here.
    if (!v.formula || !looksLikeFormula(v.formula)) {
      resolved.set(name, { name, value: v.value, kind: v.kind, formula: v.formula });
      return v.value;
    }

    if (inProgress.has(name)) {
      throw new FormulaEvalError(
        `cycle: ${[...stack, name].join(" → ")}`,
      );
    }
    inProgress.add(name);
    try {
      const parse = parseVariableInput(v.formula, known);
      if (!parse.ok) throw new FormulaEvalError(parse.error);
      // Resolve dependencies first (this is where cycles surface).
      const env: Record<string, number> = {};
      for (const dep of formulaDependencies(v.formula, known)) {
        env[dep] = resolve(dep, [...stack, name]);
      }
      const value = evalFormula(v.formula, env);
      const kind = inferFormulaKind(v.formula, byName, known);
      resolved.set(name, { name, value, kind, formula: v.formula });
      return value;
    } finally {
      inProgress.delete(name);
    }
  }

  for (const v of vars) {
    try {
      resolve(v.name, []);
    } catch (err) {
      errors.set(v.name, err instanceof Error ? err.message : String(err));
      // Park a 0 so downstream consumers still see an entry.
      resolved.set(v.name, { name: v.name, value: 0, kind: v.kind, formula: v.formula });
    }
  }
  return { resolved, errors };
}

/** A formula's dimension = the dimension of the variables it references
 *  (Length/Angle win over Number). A formula referencing only numbers is a
 *  Number. Mixed Length+Angle falls back to Number (ambiguous). */
function inferFormulaKind(
  expr: string,
  byName: Map<string, RawVariable>,
  known: ReadonlySet<string>,
): VarKind {
  let hasLength = false;
  let hasAngle = false;
  for (const dep of formulaDependencies(expr, known)) {
    const k = byName.get(dep)?.kind;
    if (k === "length") hasLength = true;
    else if (k === "angle") hasAngle = true;
  }
  if (hasLength && !hasAngle) return "length";
  if (hasAngle && !hasLength) return "angle";
  return "number";
}

export class FormulaEvalError extends Error {}

// ─── tiny formula evaluator (mirror of crates/kernel/src/expr.rs) ─────────

type Tok =
  | { t: "num"; v: number }
  | { t: "id"; v: string }
  | { t: "op"; v: string };

function tokenize(src: string): Tok[] {
  const out: Tok[] = [];
  let i = 0;
  const s = src.replace(/^=\s*/, "");
  while (i < s.length) {
    const c = s[i];
    if (c === " " || c === "\t") {
      i++;
      continue;
    }
    if ("+-*/^(),".includes(c)) {
      out.push({ t: "op", v: c });
      i++;
      continue;
    }
    if (/[0-9.]/.test(c)) {
      let j = i;
      while (j < s.length && /[0-9.eE+\-]/.test(s[j])) {
        // only consume +/- as exponent sign
        if ((s[j] === "+" || s[j] === "-") && !/[eE]/.test(s[j - 1])) break;
        j++;
      }
      out.push({ t: "num", v: Number(s.slice(i, j)) });
      i = j;
      continue;
    }
    if (/[\p{L}_]/u.test(c)) {
      let j = i;
      while (j < s.length && /[\p{L}\p{N}_]/u.test(s[j])) j++;
      out.push({ t: "id", v: s.slice(i, j) });
      i = j;
      continue;
    }
    throw new FormulaEvalError(`unexpected character \`${c}\``);
  }
  return out;
}

/** Recursive-descent eval mirroring the kernel grammar (expr/term/power/atom,
 *  function calls, builtins). Used only for the panel's live preview. */
export function evalFormula(src: string, env: Record<string, number>): number {
  const toks = tokenize(src);
  let pos = 0;
  const peek = () => toks[pos];
  const eat = () => toks[pos++];

  function expr(): number {
    let left = term();
    for (;;) {
      const p = peek();
      if (p?.t === "op" && (p.v === "+" || p.v === "-")) {
        eat();
        const r = term();
        left = p.v === "+" ? left + r : left - r;
      } else break;
    }
    return left;
  }
  function term(): number {
    let left = factor();
    for (;;) {
      const p = peek();
      if (p?.t === "op" && (p.v === "*" || p.v === "/")) {
        eat();
        const r = factor();
        if (p.v === "/" && r === 0) throw new FormulaEvalError("division by zero");
        left = p.v === "*" ? left * r : left / r;
      } else break;
    }
    return left;
  }
  function factor(): number {
    const p = peek();
    if (p?.t === "op" && p.v === "-") {
      eat();
      return -factor();
    }
    if (p?.t === "op" && p.v === "+") {
      eat();
      return factor();
    }
    return power();
  }
  function power(): number {
    const base = atom();
    const p = peek();
    if (p?.t === "op" && p.v === "^") {
      eat();
      return Math.pow(base, factor());
    }
    return base;
  }
  function atom(): number {
    const tk = eat();
    if (!tk) throw new FormulaEvalError("unexpected end of input");
    if (tk.t === "num") return tk.v;
    if (tk.t === "id") {
      if (peek()?.t === "op" && peek()?.v === "(") {
        eat();
        const args: number[] = [];
        if (!(peek()?.t === "op" && peek()?.v === ")")) {
          for (;;) {
            args.push(expr());
            if (peek()?.t === "op" && peek()?.v === ",") {
              eat();
              continue;
            }
            break;
          }
        }
        const close = eat();
        if (!(close?.t === "op" && close.v === ")"))
          throw new FormulaEvalError("missing `)`");
        return callBuiltin(tk.v.toLowerCase(), args);
      }
      if (tk.v in env) return env[tk.v];
      if (tk.v in BUILTIN_CONSTS) return BUILTIN_CONSTS[tk.v];
      throw new FormulaEvalError(`unknown variable \`${tk.v}\``);
    }
    if (tk.t === "op" && tk.v === "(") {
      const inner = expr();
      const close = eat();
      if (!(close?.t === "op" && close.v === ")"))
        throw new FormulaEvalError("missing `)`");
      return inner;
    }
    throw new FormulaEvalError(`unexpected token \`${tk.v}\``);
  }

  const result = expr();
  if (pos !== toks.length) throw new FormulaEvalError("trailing input");
  return result;
}

function callBuiltin(name: string, a: number[]): number {
  const one = (f: (x: number) => number): number => {
    if (a.length !== 1)
      throw new FormulaEvalError(`\`${name}\` takes 1 argument, got ${a.length}`);
    return f(a[0]);
  };
  const two = (f: (x: number, y: number) => number): number => {
    if (a.length !== 2)
      throw new FormulaEvalError(`\`${name}\` takes 2 arguments, got ${a.length}`);
    return f(a[0], a[1]);
  };
  switch (name) {
    case "sin": return one(Math.sin);
    case "cos": return one(Math.cos);
    case "tan": return one(Math.tan);
    case "asin": return one(Math.asin);
    case "acos": return one(Math.acos);
    case "atan": return one(Math.atan);
    case "sqrt": return one(Math.sqrt);
    case "abs": return one(Math.abs);
    case "floor": return one(Math.floor);
    case "ceil": return one(Math.ceil);
    case "round": return one(Math.round);
    case "sign": return one(Math.sign);
    case "ln": return one(Math.log);
    case "deg2rad": return one((x) => (x * Math.PI) / 180);
    case "rad2deg": return one((x) => (x * 180) / Math.PI);
    case "pow": return two(Math.pow);
    case "min": return two(Math.min);
    case "max": return two(Math.max);
    case "atan2": return two(Math.atan2);
    default:
      throw new FormulaEvalError(`unknown function \`${name}\``);
  }
}

// ─── display ──────────────────────────────────────────────────────────────

/** The base-unit suffix shown after a resolved value for its kind. */
export function baseUnitLabel(kind: VarKind): string {
  return kind === "length" ? "mm" : kind === "angle" ? "°" : "";
}

/** What to put in the field for a stored variable: its source text if it has
 *  one (formula / unit literal), else the bare resolved number. */
export function variableDisplayText(v: {
  value: number;
  kind: VarKind;
  formula?: string;
}): string {
  if (v.formula && v.formula.length > 0) return v.formula;
  return String(v.value);
}
