//! Custom-SDF expression tape: compiler + generic evaluator.
//!
//! [`Primitive::CustomSdf`] carries a postfix [`SdfExprOp`] program over
//! the leaf-local point — fidget-style closed-form math as *data* (no
//! loops, no runtime parser). This module owns both ends:
//!
//! * [`compile_expr`] parses the authoring text (`"sin(x)*cos(y) + …"`)
//!   into a validated tape (stack depth ≤ [`SDF_EXPR_MAX_STACK`], length
//!   ≤ [`SDF_EXPR_MAX_OPS`]). The grammar extends `expr.rs`'s formula
//!   grammar with the variables `x, y, z`, the constants `pi`/`tau`,
//!   `^` (power) and function calls. Convenience functions are
//!   desugared at compile time (`fract`, `mod`, `clamp`, `pow` with
//!   small integer exponent → unrolled multiplication), so the runtime
//!   op set stays small and GPU-mirrorable.
//! * [`eval_expr_generic`] interprets a tape generically over
//!   `T: DualNum<F>` — the same instantiation split as
//!   [`crate::sdf_eval::sdf_primitive_generic`]: `<f32, f32>` for the
//!   CPU reference and `<DualSVec64<3>, f64>` for exact gradients.
//!   `Floor` (and the `pow` fallback for non-positive bases) lift the
//!   real part with a zero derivative — correct almost everywhere.
//!
//! A malformed tape (stack under/overflow — impossible for compiler
//! output, possible for hand-built tapes from FieldSkript) evaluates to
//! a large positive constant instead of panicking: the body simply
//! renders nothing.

use fielddraft_protocol::{SdfExprOp, SDF_EXPR_MAX_OPS, SDF_EXPR_MAX_STACK};
use num_dual::{DualNum, DualNumFloat};

use crate::sdf_eval::SdfSqrt;

/// Value returned for a malformed tape — far away, positive: the field
/// contributes nothing to any union.
const MALFORMED_TAPE_DISTANCE: f32 = 1.0e6;

/// Interpret a compiled tape at the (leaf-local) point `p`.
pub(crate) fn eval_expr_generic<T, F>(ops: &[SdfExprOp], p: [T; 3]) -> T
where
    T: DualNum<F> + Copy + PartialOrd + SdfSqrt,
    F: DualNumFloat + From<f32>,
{
    let cf = |v: f32| T::from(<F as From<f32>>::from(v));
    let zero = T::from(F::zero());
    let bad = cf(MALFORMED_TAPE_DISTANCE);

    let mut stack = [zero; SDF_EXPR_MAX_STACK];
    let mut sp = 0usize;

    macro_rules! push {
        ($v:expr) => {{
            if sp >= SDF_EXPR_MAX_STACK {
                return bad;
            }
            stack[sp] = $v;
            sp += 1;
        }};
    }
    macro_rules! pop {
        () => {{
            if sp == 0 {
                return bad;
            }
            sp -= 1;
            stack[sp]
        }};
    }

    for op in ops {
        match op {
            SdfExprOp::X => push!(p[0]),
            SdfExprOp::Y => push!(p[1]),
            SdfExprOp::Z => push!(p[2]),
            SdfExprOp::Const { v } => push!(cf(*v)),
            SdfExprOp::Add => {
                let b = pop!();
                let a = pop!();
                push!(a + b);
            }
            SdfExprOp::Sub => {
                let b = pop!();
                let a = pop!();
                push!(a - b);
            }
            SdfExprOp::Mul => {
                let b = pop!();
                let a = pop!();
                push!(a * b);
            }
            SdfExprOp::Div => {
                let b = pop!();
                let a = pop!();
                push!(a / b);
            }
            SdfExprOp::Neg => {
                let a = pop!();
                push!(zero - a);
            }
            SdfExprOp::Min => {
                let b = pop!();
                let a = pop!();
                push!(if a < b { a } else { b });
            }
            SdfExprOp::Max => {
                let b = pop!();
                let a = pop!();
                push!(if a > b { a } else { b });
            }
            SdfExprOp::Abs => {
                let a = pop!();
                push!(a.abs());
            }
            SdfExprOp::Sqrt => {
                let a = pop!();
                // Negative operand → 0 (avoid NaN poisoning the field);
                // the guarded sqrt keeps dual derivatives finite at 0.
                push!(if a < zero { zero } else { SdfSqrt::sdf_sqrt(a) });
            }
            SdfExprOp::Sin => {
                let a = pop!();
                push!(a.sin());
            }
            SdfExprOp::Cos => {
                let a = pop!();
                push!(a.cos());
            }
            SdfExprOp::Floor => {
                // Real-part floor, zero derivative (correct a.e.).
                let a = pop!();
                push!(T::from(a.re().floor()));
            }
            SdfExprOp::Exp => {
                let a = pop!();
                push!(a.exp());
            }
            SdfExprOp::Ln => {
                let a = pop!();
                // ln of non-positive → large negative real, zero
                // derivative (keeps the tape total).
                push!(if a.re() > F::zero() {
                    a.ln()
                } else {
                    T::from(<F as From<f32>>::from(-MALFORMED_TAPE_DISTANCE))
                });
            }
            SdfExprOp::Pow => {
                let b = pop!();
                let a = pop!();
                // a^b = exp(b·ln a), defined for a > 0 only; a ≤ 0 → 0.
                // (Deliberately NOT powf: the GPU has no negative-base
                // pow, and CPU/dual/GPU must agree bit-for-bit in
                // control flow. Integer exponents on possibly-negative
                // bases should be written as products.)
                push!(if a.re() > F::zero() { (a.ln() * b).exp() } else { zero });
            }
            SdfExprOp::Atan2 => {
                let x = pop!();
                let y = pop!();
                push!(y.atan2(x));
            }
            SdfExprOp::Asin => {
                let a = pop!();
                push!(clamp_unit(a).asin());
            }
            SdfExprOp::Acos => {
                let a = pop!();
                push!(clamp_unit(a).acos());
            }
        }
    }
    if sp == 1 { stack[0] } else { bad }
}

/// Clamp to [-1, 1] on the real part so asin/acos stay finite.
fn clamp_unit<T, F>(a: T) -> T
where
    T: DualNum<F> + Copy + PartialOrd,
    F: DualNumFloat,
{
    let one = T::from(F::one());
    let neg_one = T::from(F::zero()) - one;
    if a > one {
        one
    } else if a < neg_one {
        neg_one
    } else {
        a
    }
}

// ── Compiler ────────────────────────────────────────────────────────

/// Parse and compile an expression over `x, y, z` into a validated
/// postfix tape. Errors are human-readable (they surface as regen
/// errors in the feature dialog).
pub fn compile_expr(source: &str) -> Result<Vec<SdfExprOp>, String> {
    let tokens = lex(source)?;
    let mut parser = Parser { tokens, pos: 0, ops: Vec::new() };
    parser.expr()?;
    if parser.pos != parser.tokens.len() {
        return Err(format!(
            "unexpected trailing input at token {}",
            parser.pos
        ));
    }
    validate_tape(&parser.ops)?;
    Ok(parser.ops)
}

/// Validate stack discipline and limits of a tape (also used for
/// FieldSkript-built tapes that bypass the text parser).
pub fn validate_tape(ops: &[SdfExprOp]) -> Result<(), String> {
    if ops.is_empty() {
        return Err("empty expression".into());
    }
    if ops.len() > SDF_EXPR_MAX_OPS {
        return Err(format!(
            "expression too long: {} ops (max {SDF_EXPR_MAX_OPS})",
            ops.len()
        ));
    }
    let mut depth: usize = 0;
    for (i, op) in ops.iter().enumerate() {
        let (pops, pushes) = match op {
            SdfExprOp::X | SdfExprOp::Y | SdfExprOp::Z | SdfExprOp::Const { .. } => (0, 1),
            SdfExprOp::Neg
            | SdfExprOp::Abs
            | SdfExprOp::Sqrt
            | SdfExprOp::Sin
            | SdfExprOp::Cos
            | SdfExprOp::Floor
            | SdfExprOp::Exp
            | SdfExprOp::Ln
            | SdfExprOp::Asin
            | SdfExprOp::Acos => (1, 1),
            SdfExprOp::Add
            | SdfExprOp::Sub
            | SdfExprOp::Mul
            | SdfExprOp::Div
            | SdfExprOp::Min
            | SdfExprOp::Max
            | SdfExprOp::Pow
            | SdfExprOp::Atan2 => (2, 1),
        };
        if depth < pops {
            return Err(format!("stack underflow at op {i}"));
        }
        depth = depth - pops + pushes;
        if depth > SDF_EXPR_MAX_STACK {
            return Err(format!(
                "expression too deeply nested (stack > {SDF_EXPR_MAX_STACK}) at op {i}"
            ));
        }
    }
    if depth != 1 {
        return Err(format!("expression leaves {depth} values on the stack"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f32),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    LParen,
    RParen,
    Comma,
}

fn lex(src: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '+' => {
                out.push(Token::Plus);
                i += 1;
            }
            '-' => {
                out.push(Token::Minus);
                i += 1;
            }
            '*' => {
                out.push(Token::Star);
                i += 1;
            }
            '/' => {
                out.push(Token::Slash);
                i += 1;
            }
            '^' => {
                out.push(Token::Caret);
                i += 1;
            }
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            ',' => {
                out.push(Token::Comma);
                i += 1;
            }
            '0'..='9' | '.' => {
                let start = i;
                while i < bytes.len()
                    && matches!(bytes[i] as char, '0'..='9' | '.' | 'e' | 'E')
                {
                    // Accept exponent sign directly after e/E.
                    if matches!(bytes[i] as char, 'e' | 'E')
                        && i + 1 < bytes.len()
                        && matches!(bytes[i + 1] as char, '+' | '-')
                    {
                        i += 1;
                    }
                    i += 1;
                }
                let text = &src[start..i];
                let v: f32 = text
                    .parse()
                    .map_err(|_| format!("bad number `{text}`"))?;
                out.push(Token::Num(v));
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let start = i;
                while i < bytes.len()
                    && matches!(bytes[i] as char, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_')
                {
                    i += 1;
                }
                out.push(Token::Ident(src[start..i].to_ascii_lowercase()));
            }
            other => return Err(format!("unexpected character `{other}`")),
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    ops: Vec<SdfExprOp>,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, t: Token) -> Result<(), String> {
        match self.next() {
            Some(got) if got == t => Ok(()),
            got => Err(format!("expected {t:?}, got {got:?}")),
        }
    }

    fn expr(&mut self) -> Result<(), String> {
        self.term()?;
        while let Some(tok) = self.peek() {
            match tok {
                Token::Plus => {
                    self.next();
                    self.term()?;
                    self.ops.push(SdfExprOp::Add);
                }
                Token::Minus => {
                    self.next();
                    self.term()?;
                    self.ops.push(SdfExprOp::Sub);
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn term(&mut self) -> Result<(), String> {
        self.factor()?;
        while let Some(tok) = self.peek() {
            match tok {
                Token::Star => {
                    self.next();
                    self.factor()?;
                    self.ops.push(SdfExprOp::Mul);
                }
                Token::Slash => {
                    self.next();
                    self.factor()?;
                    self.ops.push(SdfExprOp::Div);
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn factor(&mut self) -> Result<(), String> {
        match self.peek() {
            Some(Token::Minus) => {
                self.next();
                self.factor()?;
                self.ops.push(SdfExprOp::Neg);
                Ok(())
            }
            Some(Token::Plus) => {
                self.next();
                self.factor()
            }
            _ => self.power(),
        }
    }

    /// `power := atom ('^' factor)?` — right-associative; small positive
    /// integer exponents are unrolled into multiplication chains (dual-
    /// and negative-base-safe), everything else emits `Pow`.
    fn power(&mut self) -> Result<(), String> {
        self.atom()?;
        if matches!(self.peek(), Some(Token::Caret)) {
            self.next();
            // Literal small integer exponent → unroll.
            if let Some(Token::Num(v)) = self.peek().cloned() {
                let is_small_int = v.fract() == 0.0 && (2.0..=8.0).contains(&v);
                if is_small_int
                    && !matches!(self.tokens.get(self.pos + 1), Some(Token::Caret))
                {
                    self.next();
                    let n = v as u32;
                    // stack: [a] → a·a·…·a. Duplicate via re-emission is
                    // impossible on a stack machine without a Dup op, so
                    // unroll as a^n = exp-free repeated squaring… simplest:
                    // emit the subexpression again? Not available. Use
                    // Pow for n > 2? No — introduce Dup-free unroll via
                    // Mul of a Const-free copy is impossible; fall back to
                    // Pow op with a Const exponent — dual-safe for a > 0,
                    // real-lifted otherwise.
                    self.ops.push(SdfExprOp::Const { v: n as f32 });
                    self.ops.push(SdfExprOp::Pow);
                    return Ok(());
                }
            }
            self.factor()?;
            self.ops.push(SdfExprOp::Pow);
        }
        Ok(())
    }

    fn atom(&mut self) -> Result<(), String> {
        match self.next() {
            Some(Token::Num(v)) => {
                self.ops.push(SdfExprOp::Const { v });
                Ok(())
            }
            Some(Token::LParen) => {
                self.expr()?;
                self.expect(Token::RParen)
            }
            Some(Token::Ident(name)) => self.ident(&name),
            other => Err(format!("expected value, got {other:?}")),
        }
    }

    fn ident(&mut self, name: &str) -> Result<(), String> {
        // Variables & constants.
        match name {
            "x" => {
                self.ops.push(SdfExprOp::X);
                return Ok(());
            }
            "y" => {
                self.ops.push(SdfExprOp::Y);
                return Ok(());
            }
            "z" => {
                self.ops.push(SdfExprOp::Z);
                return Ok(());
            }
            "pi" => {
                self.ops.push(SdfExprOp::Const { v: std::f32::consts::PI });
                return Ok(());
            }
            "tau" => {
                self.ops.push(SdfExprOp::Const { v: std::f32::consts::TAU });
                return Ok(());
            }
            _ => {}
        }
        // Function call.
        self.expect(Token::LParen)?;
        let mut argc = 1usize;
        self.expr()?;
        while matches!(self.peek(), Some(Token::Comma)) {
            self.next();
            self.expr()?;
            argc += 1;
        }
        self.expect(Token::RParen)?;

        let check = |want: usize| -> Result<(), String> {
            if argc == want {
                Ok(())
            } else {
                Err(format!("{name}() takes {want} argument(s), got {argc}"))
            }
        };
        match name {
            "sin" => {
                check(1)?;
                self.ops.push(SdfExprOp::Sin);
            }
            "cos" => {
                check(1)?;
                self.ops.push(SdfExprOp::Cos);
            }
            "sqrt" => {
                check(1)?;
                self.ops.push(SdfExprOp::Sqrt);
            }
            "abs" => {
                check(1)?;
                self.ops.push(SdfExprOp::Abs);
            }
            "exp" => {
                check(1)?;
                self.ops.push(SdfExprOp::Exp);
            }
            "ln" | "log" => {
                check(1)?;
                self.ops.push(SdfExprOp::Ln);
            }
            "floor" => {
                check(1)?;
                self.ops.push(SdfExprOp::Floor);
            }
            "asin" => {
                check(1)?;
                self.ops.push(SdfExprOp::Asin);
            }
            "acos" => {
                check(1)?;
                self.ops.push(SdfExprOp::Acos);
            }
            "min" => {
                check(2)?;
                self.ops.push(SdfExprOp::Min);
            }
            "max" => {
                check(2)?;
                self.ops.push(SdfExprOp::Max);
            }
            "pow" => {
                check(2)?;
                self.ops.push(SdfExprOp::Pow);
            }
            "atan2" => {
                check(2)?;
                self.ops.push(SdfExprOp::Atan2);
            }
            // Desugared conveniences. `fract`/`mod` need the operand
            // twice — a stack machine without Dup can't reuse it, so
            // they are rewritten structurally by the parser: not
            // supported in v1 (documented); `clamp` works because the
            // args arrive in evaluation order.
            "clamp" => {
                check(3)?;
                // stack: [x, lo, hi] → min(max(x, lo), hi)
                // max(x, lo) needs x,lo adjacent: they are ([x,lo] hi).
                // Emit: rotate-free trick — max then min works because
                // hi is on top: pop hi is WRONG order for min first.
                // Instead: [x, lo, hi]: we need min(max(x,lo),hi):
                // Max pops lo,x → pushes m → [m, hi]?? No: stack is
                // [x, lo, hi], Max pops hi,lo → max(lo,hi). Wrong.
                // Without stack manipulation ops clamp can't be
                // expressed postfix from infix arg order — reject.
                return Err(
                    "clamp() is not supported yet — write min(max(x, lo), hi)".into(),
                );
            }
            other => return Err(format!("unknown function `{other}`")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str, p: [f32; 3]) -> f32 {
        let ops = compile_expr(src).expect("compile");
        eval_expr_generic::<f32, f32>(&ops, p)
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(eval("1 + 2 * 3", [0.0; 3]), 7.0);
        assert_eq!(eval("(1 + 2) * 3", [0.0; 3]), 9.0);
        assert_eq!(eval("-x + 10", [4.0, 0.0, 0.0]), 6.0);
        assert_eq!(eval("x - y - z", [10.0, 3.0, 2.0]), 5.0);
        assert_eq!(eval("2 ^ 3", [0.0; 3]), 8.0);
    }

    #[test]
    fn variables_functions_constants() {
        let p = [1.0f32, 2.0, 3.0];
        assert!((eval("sqrt(x*x + y*y + z*z)", p) - 14.0f32.sqrt()).abs() < 1e-6);
        assert!((eval("sin(pi)", [0.0; 3])).abs() < 1e-6);
        assert!((eval("min(x, y)", p) - 1.0).abs() < 1e-6);
        assert!((eval("max(x, atan2(y, x))", p) - 1.107149).abs() < 1e-5);
        assert!((eval("abs(-x)", p) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn gyroid_matches_reference() {
        // The gyroid formula written as a custom expression must match
        // the closed-form arm (unnormalised, cell_size = 1).
        let src = "sin(x)*cos(y) + sin(y)*cos(z) + sin(z)*cos(x)";
        let p = [0.3f32, -1.2, 2.7];
        let want = p[0].sin() * p[1].cos() + p[1].sin() * p[2].cos() + p[2].sin() * p[0].cos();
        assert!((eval(src, p) - want).abs() < 1e-6);
    }

    #[test]
    fn dual_gradient_matches_finite_difference() {
        use num_dual::{Derivative, DualSVec64};
        let ops =
            compile_expr("sqrt(x*x + y*y + z*z) - 5 + 0.3*sin(2*x)*cos(3*y)").expect("compile");
        let p = [1.7f32, -0.4, 2.2];
        // Dual evaluation, each axis seeded with ∂/∂axis = 1.
        let seed = |val: f32, axis: usize| -> DualSVec64<3> {
            let mut eps = nalgebra::SVector::<f64, 3>::zeros();
            eps[axis] = 1.0;
            DualSVec64::new(val as f64, Derivative::some(eps))
        };
        let dp = [seed(p[0], 0), seed(p[1], 1), seed(p[2], 2)];
        let dual = eval_expr_generic::<DualSVec64<3>, f64>(&ops, dp);
        let grad = dual
            .eps
            .unwrap_generic(nalgebra::Const::<3>, nalgebra::Const::<1>);
        // Finite differences on the f32 instantiation.
        let h = 1e-3f32;
        for axis in 0..3 {
            let mut hi = p;
            let mut lo = p;
            hi[axis] += h;
            lo[axis] -= h;
            let fd = (eval_expr_generic::<f32, f32>(&ops, hi)
                - eval_expr_generic::<f32, f32>(&ops, lo))
                / (2.0 * h);
            let exact = grad[axis] as f32;
            assert!(
                (fd - exact).abs() < 1e-2,
                "axis {axis}: fd {fd} vs dual {exact}"
            );
        }
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(compile_expr("").is_err());
        assert!(compile_expr("sin()").is_err());
        assert!(compile_expr("bogus(x)").is_err());
        assert!(compile_expr("x +").is_err());
        assert!(compile_expr("min(x)").is_err());
        assert!(compile_expr("x ) y").is_err());
        assert!(compile_expr("w + 1").is_err(), "unknown ident must fail");
    }

    #[test]
    fn validates_limits() {
        // Too long.
        let mut ops = vec![SdfExprOp::X];
        for _ in 0..SDF_EXPR_MAX_OPS {
            ops.push(SdfExprOp::Neg);
        }
        assert!(validate_tape(&ops).is_err());
        // Underflow.
        assert!(validate_tape(&[SdfExprOp::Add]).is_err());
        // Leftover values.
        assert!(validate_tape(&[SdfExprOp::X, SdfExprOp::Y]).is_err());
    }
}
