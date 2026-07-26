/**
 * Onshape expression evaluation (P3-Fundament, #409/#415/#417).
 *
 * Onshape stores every quantity parameter as the AUTHORED TEXT
 * (`"25 cm"`, `"3 mm+#HoleExtraGap"`, `"#TopInnerDiameter/2+3cm"`), and only
 * sometimes as an evaluated SI number. The import therefore has to evaluate
 * that text itself — against the document variables an `assignVariable`
 * feature declared earlier in the same feature list.
 *
 * The evaluator is UNIT-AWARE, because the corpus is: `1/cm` is an inverse
 * length that makes `x * 1/cm` dimensionless, `(…) cm` multiplies a
 * parenthesised group by a centimetre, and `5 mm + #HoleExtraGap` only adds up
 * because both sides are lengths. Every value is therefore a {@link Quantity}
 * — a number plus a length and an angle exponent — carried in the module's
 * BASE units: **millimetres** and **degrees**.
 *
 * Deliberately NOT done here: `eval()`, or anything that reaches outside this
 * module. The grammar is a hand-written recursive-descent parser mirroring
 * Onshape's:
 *
 *     expr   := term (('+' | '-') term)*
 *     term   := unary (('*' | '/') unary)*
 *     unary  := ('+' | '-') unary | factor
 *     factor := power power*                           // juxtaposition = '*'
 *     power  := atom ('^' unary)?
 *     atom   := number | '#'ident | ident ['(' expr (',' expr)* ')'] | '(' expr ')'
 *
 * Juxtaposition is what makes `2mm`, `1.5 deg` and `(…) cm` work: a literal
 * unit is simply a quantity of value 1, and writing two factors next to each
 * other multiplies them. It binds TIGHTER than `*` and `/`, because `2 mm` is
 * one literal — `4 mm / 2 mm` is the ratio 2, not an area.
 *
 * FieldDraft's own formula language (`web/src/variableInput.ts`,
 * `crates/kernel/src/expr.rs`) is DIMENSIONLESS and uses bare identifiers, so
 * it cannot host these expressions directly — but it can host their
 * base-unit projection. {@link toFieldDraftFormula} renders exactly the subset
 * where that projection is faithful, and the importer cross-checks the result
 * against this evaluator before storing it.
 */

/** A number with a length and an angle exponent. Base units: MILLIMETRE and
 *  DEGREE — the same base FieldDraft stores its own document variables in, so
 *  `45 deg` round-trips exactly instead of through a radian conversion. */
export interface Quantity {
  v: number;
  /** Length exponent: 1 = length, 2 = area, −1 = inverse length, 0 = plain. */
  len: number;
  /** Angle exponent. */
  ang: number;
}

/** A parse/evaluation failure that names its cause in user-facing German. */
export class OnshapeExprError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "OnshapeExprError";
  }
}

export const NUMBER: Quantity = { v: 1, len: 0, ang: 0 };

const RAD_PER_DEG = Math.PI / 180;
const DEG_PER_RAD = 180 / Math.PI;

/** Unit literals → their value in base units (mm / degree). */
const UNITS: Record<string, Quantity> = {
  mm: { v: 1, len: 1, ang: 0 },
  millimeter: { v: 1, len: 1, ang: 0 },
  millimeters: { v: 1, len: 1, ang: 0 },
  millimetre: { v: 1, len: 1, ang: 0 },
  millimetres: { v: 1, len: 1, ang: 0 },
  cm: { v: 10, len: 1, ang: 0 },
  centimeter: { v: 10, len: 1, ang: 0 },
  centimeters: { v: 10, len: 1, ang: 0 },
  centimetre: { v: 10, len: 1, ang: 0 },
  centimetres: { v: 10, len: 1, ang: 0 },
  m: { v: 1000, len: 1, ang: 0 },
  meter: { v: 1000, len: 1, ang: 0 },
  meters: { v: 1000, len: 1, ang: 0 },
  metre: { v: 1000, len: 1, ang: 0 },
  metres: { v: 1000, len: 1, ang: 0 },
  in: { v: 25.4, len: 1, ang: 0 },
  inch: { v: 25.4, len: 1, ang: 0 },
  inches: { v: 25.4, len: 1, ang: 0 },
  ft: { v: 304.8, len: 1, ang: 0 },
  foot: { v: 304.8, len: 1, ang: 0 },
  feet: { v: 304.8, len: 1, ang: 0 },
  yd: { v: 914.4, len: 1, ang: 0 },
  yard: { v: 914.4, len: 1, ang: 0 },
  deg: { v: 1, len: 0, ang: 1 },
  degree: { v: 1, len: 0, ang: 1 },
  degrees: { v: 1, len: 0, ang: 1 },
  "°": { v: 1, len: 0, ang: 1 },
  rad: { v: DEG_PER_RAD, len: 0, ang: 1 },
  radian: { v: DEG_PER_RAD, len: 0, ang: 1 },
  radians: { v: DEG_PER_RAD, len: 0, ang: 1 },
};

const CONSTANTS: Record<string, Quantity> = {
  pi: { v: Math.PI, len: 0, ang: 0 },
  PI: { v: Math.PI, len: 0, ang: 0 },
};

// ─── Tokenizer ──────────────────────────────────────────────────

type Tok =
  | { t: "num"; v: number }
  | { t: "id"; v: string }
  | { t: "var"; v: string }
  | { t: "op"; v: string };

/** Identifier characters — Onshape variable names may hold umlauts, so every
 *  non-ASCII code point counts as a letter (same rule as the kernel lexer). */
function isIdentStart(c: string): boolean {
  return /[A-Za-z_]/.test(c) || c === "°" || c.charCodeAt(0) >= 0x80;
}
function isIdentPart(c: string): boolean {
  return isIdentStart(c) || /[0-9]/.test(c);
}

function tokenize(src: string): Tok[] {
  const out: Tok[] = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (/\s/.test(c)) {
      i += 1;
      continue;
    }
    if (/[0-9.]/.test(c)) {
      const m = /^(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?/.exec(src.slice(i));
      if (!m) throw new OnshapeExprError(`unlesbare Zahl bei "${src.slice(i, i + 8)}"`);
      out.push({ t: "num", v: parseFloat(m[0]) });
      i += m[0].length;
      continue;
    }
    if (c === "#") {
      let j = i + 1;
      while (j < src.length && isIdentPart(src[j])) j += 1;
      if (j === i + 1) throw new OnshapeExprError(`"#" ohne Variablenname`);
      out.push({ t: "var", v: src.slice(i + 1, j) });
      i = j;
      continue;
    }
    if (isIdentStart(c)) {
      let j = i;
      while (j < src.length && isIdentPart(src[j])) j += 1;
      out.push({ t: "id", v: src.slice(i, j) });
      i = j;
      continue;
    }
    if ("+-*/^(),".includes(c)) {
      out.push({ t: "op", v: c });
      i += 1;
      continue;
    }
    throw new OnshapeExprError(`unerwartetes Zeichen "${c}"`);
  }
  return out;
}

// ─── AST ────────────────────────────────────────────────────────

type Node =
  | { k: "num"; v: number }
  | { k: "ident"; name: string }
  | { k: "var"; name: string }
  | { k: "bin"; op: string; l: Node; r: Node }
  | { k: "neg"; x: Node }
  | { k: "call"; name: string; args: Node[] };

function parse(src: string): Node {
  const toks = tokenize(src);
  let p = 0;
  const peek = (): Tok | undefined => toks[p];
  const isOp = (v: string): boolean => {
    const t = peek();
    return !!t && t.t === "op" && t.v === v;
  };
  const eat = (v: string): void => {
    if (!isOp(v)) throw new OnshapeExprError(`"${v}" erwartet`);
    p += 1;
  };

  const startsFactor = (): boolean => {
    const t = peek();
    if (!t) return false;
    return t.t === "num" || t.t === "id" || t.t === "var" || (t.t === "op" && t.v === "(");
  };

  function atom(): Node {
    const t = peek();
    if (!t) throw new OnshapeExprError("Ausdruck bricht ab");
    if (t.t === "num") {
      p += 1;
      return { k: "num", v: t.v };
    }
    if (t.t === "var") {
      p += 1;
      return { k: "var", name: t.v };
    }
    if (t.t === "id") {
      p += 1;
      if (isOp("(")) {
        eat("(");
        const args: Node[] = [expr()];
        while (isOp(",")) {
          p += 1;
          args.push(expr());
        }
        eat(")");
        return { k: "call", name: t.v, args };
      }
      return { k: "ident", name: t.v };
    }
    if (t.t === "op" && t.v === "(") {
      eat("(");
      const inner = expr();
      eat(")");
      return inner;
    }
    throw new OnshapeExprError(`unerwartetes "${t.v}"`);
  }

  function power(): Node {
    const base = atom();
    if (isOp("^")) {
      p += 1;
      return { k: "bin", op: "^", l: base, r: unary() };
    }
    return base;
  }

  /** Juxtaposition IS multiplication — and binds tighter than `*` / `/`, so
   *  `2 mm` is a single literal and `4 mm / 2 mm` is a ratio, not an area. */
  function factor(): Node {
    let left = power();
    while (startsFactor()) left = { k: "bin", op: "*", l: left, r: power() };
    return left;
  }

  function unary(): Node {
    if (isOp("-")) {
      p += 1;
      return { k: "neg", x: unary() };
    }
    if (isOp("+")) {
      p += 1;
      return unary();
    }
    return factor();
  }

  function term(): Node {
    let left = unary();
    while (isOp("*") || isOp("/")) {
      const op = (peek() as { t: "op"; v: string }).v;
      p += 1;
      left = { k: "bin", op, l: left, r: unary() };
    }
    return left;
  }

  function expr(): Node {
    let left = term();
    while (isOp("+") || isOp("-")) {
      const op = (peek() as { t: "op"; v: string }).v;
      p += 1;
      left = { k: "bin", op, l: left, r: term() };
    }
    return left;
  }

  const root = expr();
  if (p !== toks.length) {
    const t = toks[p] as Tok;
    throw new OnshapeExprError(`Rest nach dem Ausdruck: "${t.t === "op" ? t.v : String((t as any).v)}"`);
  }
  return root;
}

// ─── Evaluation ─────────────────────────────────────────────────

const isPlain = (q: Quantity): boolean => q.len === 0 && q.ang === 0;

function dimName(q: Quantity): string {
  if (isPlain(q)) return "dimensionslos";
  const parts: string[] = [];
  if (q.len) parts.push(`Länge^${q.len}`);
  if (q.ang) parts.push(`Winkel^${q.ang}`);
  return parts.join("·");
}

function requirePlain(q: Quantity, what: string): number {
  if (!isPlain(q)) {
    throw new OnshapeExprError(`${what} erwartet eine einheitenlose Zahl, bekam ${dimName(q)}`);
  }
  return q.v;
}

/** The angle argument of a trig function, converted to RADIANS for `Math`.
 *  A dimensionless value is read as radians already (Onshape insists on a
 *  unit; the laxer form is accepted because the convention is unambiguous). */
function angleArg(q: Quantity, fn: string): number {
  if (q.ang === 1 && q.len === 0) return q.v * RAD_PER_DEG;
  if (isPlain(q)) return q.v;
  throw new OnshapeExprError(`${fn}() erwartet einen Winkel, bekam ${dimName(q)}`);
}

function sameDim(a: Quantity, b: Quantity): boolean {
  return a.len === b.len && a.ang === b.ang;
}

function evalNode(n: Node, vars: ReadonlyMap<string, Quantity>): Quantity {
  switch (n.k) {
    case "num":
      return { v: n.v, len: 0, ang: 0 };
    case "var": {
      const q = vars.get(n.name);
      if (!q) {
        throw new OnshapeExprError(`unbekannte Variable "#${n.name}"`);
      }
      return q;
    }
    case "ident": {
      const u = UNITS[n.name] ?? UNITS[n.name.toLowerCase()];
      if (u) return u;
      const c = CONSTANTS[n.name];
      if (c) return c;
      throw new OnshapeExprError(`unbekannte Einheit oder Konstante "${n.name}"`);
    }
    case "neg": {
      const q = evalNode(n.x, vars);
      return { v: -q.v, len: q.len, ang: q.ang };
    }
    case "bin": {
      const a = evalNode(n.l, vars);
      const b = evalNode(n.r, vars);
      switch (n.op) {
        case "+":
        case "-": {
          // A literal 0 is dimension-agnostic — Onshape writes `0` for an unset
          // length just as happily as `0 mm`.
          if (!sameDim(a, b) && a.v !== 0 && b.v !== 0) {
            throw new OnshapeExprError(
              `"${n.op}" zwischen ${dimName(a)} und ${dimName(b)} — Einheiten passen nicht zusammen`,
            );
          }
          const dim = isPlain(a) && a.v === 0 ? b : a;
          return { v: n.op === "+" ? a.v + b.v : a.v - b.v, len: dim.len, ang: dim.ang };
        }
        case "*":
          return { v: a.v * b.v, len: a.len + b.len, ang: a.ang + b.ang };
        case "/":
          if (b.v === 0) throw new OnshapeExprError("Division durch 0");
          return { v: a.v / b.v, len: a.len - b.len, ang: a.ang - b.ang };
        case "^": {
          const e = requirePlain(b, "Der Exponent");
          return { v: Math.pow(a.v, e), len: a.len * e, ang: a.ang * e };
        }
        default:
          throw new OnshapeExprError(`unbekannter Operator "${n.op}"`);
      }
    }
    case "call": {
      const args = n.args.map((a) => evalNode(a, vars));
      return evalCall(n.name, args);
    }
  }
}

function evalCall(rawName: string, args: Quantity[]): Quantity {
  const name = rawName.toLowerCase();
  const one = (): Quantity => {
    if (args.length !== 1) throw new OnshapeExprError(`${name}() erwartet 1 Argument`);
    return args[0];
  };
  const plainOne = (): number => requirePlain(one(), `${name}()`);
  switch (name) {
    case "sin":
      return { v: Math.sin(angleArg(one(), name)), len: 0, ang: 0 };
    case "cos":
      return { v: Math.cos(angleArg(one(), name)), len: 0, ang: 0 };
    case "tan":
      return { v: Math.tan(angleArg(one(), name)), len: 0, ang: 0 };
    case "asin":
      return { v: Math.asin(plainOne()) * DEG_PER_RAD, len: 0, ang: 1 };
    case "acos":
      return { v: Math.acos(plainOne()) * DEG_PER_RAD, len: 0, ang: 1 };
    case "atan":
      return { v: Math.atan(plainOne()) * DEG_PER_RAD, len: 0, ang: 1 };
    case "atan2": {
      if (args.length !== 2) throw new OnshapeExprError("atan2() erwartet 2 Argumente");
      if (!sameDim(args[0], args[1])) {
        throw new OnshapeExprError("atan2() erwartet zwei Werte gleicher Einheit");
      }
      return { v: Math.atan2(args[0].v, args[1].v) * DEG_PER_RAD, len: 0, ang: 1 };
    }
    case "sqrt": {
      const q = one();
      if (q.v < 0) throw new OnshapeExprError("sqrt() eines negativen Werts");
      return { v: Math.sqrt(q.v), len: q.len / 2, ang: q.ang / 2 };
    }
    case "abs": {
      const q = one();
      return { v: Math.abs(q.v), len: q.len, ang: q.ang };
    }
    case "sign":
      return { v: Math.sign(one().v), len: 0, ang: 0 };
    case "floor":
      return { v: Math.floor(plainOne()), len: 0, ang: 0 };
    case "ceil":
      return { v: Math.ceil(plainOne()), len: 0, ang: 0 };
    case "ln":
      return { v: Math.log(plainOne()), len: 0, ang: 0 };
    case "log":
      return { v: Math.log10(plainOne()), len: 0, ang: 0 };
    case "exp":
      return { v: Math.exp(plainOne()), len: 0, ang: 0 };
    case "round": {
      // 1 argument: nearest integer (dimensionless, as in Onshape).
      // 2 arguments: `round(x, p)` snaps x to the nearest MULTIPLE of the
      // precision p — which is unit-safe (x and p must share a dimension) and
      // is what the corpus uses (`round(…, .1)` → one decimal).
      if (args.length === 1) return { v: Math.round(plainOne()), len: 0, ang: 0 };
      if (args.length !== 2) throw new OnshapeExprError("round() erwartet 1 oder 2 Argumente");
      const [x, prec] = args;
      if (!sameDim(x, prec)) {
        throw new OnshapeExprError("round(x, p): x und p müssen dieselbe Einheit haben");
      }
      if (prec.v === 0) throw new OnshapeExprError("round(x, 0) ist nicht definiert");
      return { v: Math.round(x.v / prec.v) * prec.v, len: x.len, ang: x.ang };
    }
    case "pow": {
      if (args.length !== 2) throw new OnshapeExprError("pow() erwartet 2 Argumente");
      const e = requirePlain(args[1], "Der Exponent von pow()");
      return { v: Math.pow(args[0].v, e), len: args[0].len * e, ang: args[0].ang * e };
    }
    case "min":
    case "max": {
      if (args.length < 1) throw new OnshapeExprError(`${name}() erwartet Argumente`);
      for (const a of args) {
        if (!sameDim(a, args[0])) {
          throw new OnshapeExprError(`${name}() erwartet Werte gleicher Einheit`);
        }
      }
      const v = name === "min" ? Math.min(...args.map((a) => a.v)) : Math.max(...args.map((a) => a.v));
      return { v, len: args[0].len, ang: args[0].ang };
    }
    default:
      throw new OnshapeExprError(`unbekannte Funktion "${rawName}()"`);
  }
}

/** Evaluate an Onshape expression against a variable table. Throws
 *  {@link OnshapeExprError} with a German cause — callers turn that into a
 *  named import warning. */
export function evalOnshapeExpression(
  src: string,
  vars: ReadonlyMap<string, Quantity> = new Map(),
): Quantity {
  const text = src.trim();
  if (!text) throw new OnshapeExprError("leerer Ausdruck");
  // Onshape writes `"?"` for a MEASURED variable whose value only exists
  // inside its own geometry evaluation — there is nothing to parse.
  if (text === '"?"' || text === "?") {
    throw new OnshapeExprError("gemessener Wert (Onshape liefert nur \"?\")");
  }
  const q = evalNode(parse(text), vars);
  if (!Number.isFinite(q.v)) throw new OnshapeExprError("Ergebnis ist keine endliche Zahl");
  return q;
}

/** Non-throwing wrapper. */
export function tryEvalOnshapeExpression(
  src: string,
  vars: ReadonlyMap<string, Quantity> = new Map(),
): { ok: true; q: Quantity } | { ok: false; error: string } {
  try {
    return { ok: true, q: evalOnshapeExpression(src, vars) };
  } catch (e) {
    return { ok: false, error: e instanceof OnshapeExprError ? e.message : String(e) };
  }
}

/** mm of a LENGTH quantity, or `null` when it is not a length. A plain zero
 *  counts as 0 mm (Onshape writes a bare `0` for an unset length). */
export function quantityToMm(q: Quantity): number | null {
  if (q.len === 1 && q.ang === 0) return q.v;
  if (isPlain(q) && q.v === 0) return 0;
  return null;
}

/** Degrees of an ANGLE quantity, or `null` when it is not an angle. */
export function quantityToDeg(q: Quantity): number | null {
  if (q.ang === 1 && q.len === 0) return q.v;
  if (isPlain(q) && q.v === 0) return 0;
  return null;
}

/** Which FieldDraft variable kind a quantity fits, or `null` for a dimension
 *  FieldDraft has no kind for (an area, an inverse length, …). */
export function quantityKind(q: Quantity): "length" | "angle" | "number" | null {
  if (isPlain(q)) return "number";
  if (q.len === 1 && q.ang === 0) return "length";
  if (q.ang === 1 && q.len === 0) return "angle";
  return null;
}

/** A quantity's human description, for warnings. */
export function describeDimension(q: Quantity): string {
  return dimName(q);
}

// ─── Projection onto FieldDraft's formula language ──────────────

/**
 * Render an Onshape expression as a FieldDraft formula (`variableInput.ts` /
 * `crates/kernel/src/expr.rs`), or `null` when that projection would not be
 * faithful.
 *
 * FieldDraft formulas are DIMENSIONLESS and read their variables in base units
 * (mm for a length, DEGREES for an angle). A pure-length expression therefore
 * projects exactly — every unit literal becomes its millimetre factor — and
 * that is the subset rendered here:
 *
 *  - no function calls and no `^` (a trig call would need radians, which
 *    collides with FieldDraft's degree base),
 *  - no angle unit anywhere in the expression,
 *  - every `#Name` maps to a known FieldDraft variable name — `fdName` returns
 *    `undefined` for anything the caller does not want projected (an ANGLE
 *    variable, a variable that failed to evaluate), which vetoes the render.
 *
 * The caller must still cross-check the rendered text against
 * {@link evalOnshapeExpression} before storing it — see `onshapeImport.ts`.
 */
export function toFieldDraftFormula(
  src: string,
  fdName: (onshapeVar: string) => string | undefined,
): string | null {
  let root: Node;
  try {
    root = parse(src.trim());
  } catch {
    return null;
  }
  const render = (n: Node): string | null => {
    switch (n.k) {
      case "num":
        return String(n.v);
      case "var": {
        const name = fdName(n.name);
        return name ?? null;
      }
      case "ident": {
        const u = UNITS[n.name] ?? UNITS[n.name.toLowerCase()];
        if (u && u.len === 1 && u.ang === 0) return String(u.v);
        if (CONSTANTS[n.name]) return String(CONSTANTS[n.name].v);
        return null;
      }
      case "neg": {
        const x = render(n.x);
        return x === null ? null : `-(${x})`;
      }
      case "bin": {
        if (n.op === "^") return null;
        const l = render(n.l);
        const r = render(n.r);
        if (l === null || r === null) return null;
        return `(${l} ${n.op} ${r})`;
      }
      case "call":
        return null;
    }
  };
  return render(root);
}
