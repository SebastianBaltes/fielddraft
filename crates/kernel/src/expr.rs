//! K8.11: Minimal expression language for document-level
//! variable references in `Length`/`Angle`/`f32` feature
//! parameters.
//!
//! Grammar (v0):
//!
//! ```text
//! expr    := term (('+' | '-') term)*
//! term    := factor (('*' | '/') factor)*
//! factor  := ('-' | '+') factor | atom
//! atom    := number | ident | '(' expr ')'
//! number  := ('0'..'9')+ ('.' ('0'..'9')+)?
//! ident   := ('a'..'z' | 'A'..'Z' | '_') ('a'..'z' | 'A'..'Z' | '0'..'9' | '_')*
//! ```
//!
//! Evaluation resolves identifiers against a
//! `&BTreeMap<String, f32>`. Everything is f32 and
//! dimensionless — the K8.11 slice does not yet unit-check
//! arithmetic (that's planned for later). A formula that
//! references an unknown identifier returns an error so the
//! kernel can surface a regen failure rather than silently
//! producing garbage.
//!
//! The entry point used by the feature registry is
//! [`resolve_formula`], which takes the wire-form string
//! (`"=width / 2 + 5"`) and the document's variables table
//! and returns the resolved `f32`. The leading `=` is
//! mandatory — a plain numeric string is *not* a formula
//! and should never reach the resolver in the first place.

use std::collections::BTreeMap;

/// Errors produced by the expression parser / evaluator.
#[derive(Debug, Clone, PartialEq)]
pub enum FormulaError {
    /// The input didn't start with the mandatory `=` sigil.
    MissingSigil,
    /// An unknown identifier was referenced at evaluation
    /// time. Carries the offending name so the caller can
    /// surface it in the diagnostic.
    UnknownIdent(String),
    /// A syntactic or lexical problem. The payload is a
    /// short human-readable hint.
    Parse(String),
    /// Division by zero at evaluation time.
    DivisionByZero,
}

impl std::fmt::Display for FormulaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSigil => write!(f, "formula must start with `=`"),
            Self::UnknownIdent(name) => write!(f, "unknown variable `{name}`"),
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
            Self::DivisionByZero => write!(f, "division by zero"),
        }
    }
}

impl std::error::Error for FormulaError {}

/// Parse and evaluate a formula string (e.g. `"=width / 2"`)
/// against the document's variable table. The sigil is
/// mandatory; the caller already decided the field carries
/// a formula rather than a literal number.
pub fn resolve_formula(
    text: &str,
    vars: &BTreeMap<String, f32>,
) -> Result<f32, FormulaError> {
    let body = text
        .strip_prefix('=')
        .ok_or(FormulaError::MissingSigil)?
        .trim();
    if body.is_empty() {
        return Err(FormulaError::Parse("empty formula body".to_string()));
    }
    let mut tokens = Lexer::new(body).tokenise()?;
    let mut parser = Parser {
        tokens: &mut tokens,
        pos: 0,
    };
    let ast = parser.parse_expr()?;
    if parser.pos != parser.tokens.len() {
        return Err(FormulaError::Parse(format!(
            "unexpected trailing input at position {}",
            parser.pos
        )));
    }
    ast.eval(vars)
}

// ─── lexer ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f32),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
        }
    }

    fn tokenise(mut self) -> Result<Vec<Token>, FormulaError> {
        let mut out = Vec::new();
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            match c {
                b' ' | b'\t' => {
                    self.pos += 1;
                }
                b'+' => {
                    out.push(Token::Plus);
                    self.pos += 1;
                }
                b'-' => {
                    out.push(Token::Minus);
                    self.pos += 1;
                }
                b'*' => {
                    out.push(Token::Star);
                    self.pos += 1;
                }
                b'/' => {
                    out.push(Token::Slash);
                    self.pos += 1;
                }
                b'(' => {
                    out.push(Token::LParen);
                    self.pos += 1;
                }
                b')' => {
                    out.push(Token::RParen);
                    self.pos += 1;
                }
                b'0'..=b'9' | b'.' => {
                    out.push(self.number()?);
                }
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                    out.push(self.ident());
                }
                other => {
                    return Err(FormulaError::Parse(format!(
                        "unexpected character `{}` at position {}",
                        other as char, self.pos
                    )));
                }
            }
        }
        Ok(out)
    }

    fn number(&mut self) -> Result<Token, FormulaError> {
        let start = self.pos;
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c.is_ascii_digit() || c == b'.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let slice = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|_| FormulaError::Parse("invalid utf-8 in number".to_string()))?;
        slice
            .parse::<f32>()
            .map(Token::Number)
            .map_err(|e| FormulaError::Parse(format!("bad number `{slice}`: {e}")))
    }

    fn ident(&mut self) -> Token {
        let start = self.pos;
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let slice = std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");
        Token::Ident(slice.to_string())
    }
}

// ─── AST + parser ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Lit(f32),
    Var(String),
    Neg(Box<Node>),
    Add(Box<Node>, Box<Node>),
    Sub(Box<Node>, Box<Node>),
    Mul(Box<Node>, Box<Node>),
    Div(Box<Node>, Box<Node>),
}

impl Node {
    fn eval(&self, vars: &BTreeMap<String, f32>) -> Result<f32, FormulaError> {
        match self {
            Node::Lit(n) => Ok(*n),
            Node::Var(name) => vars
                .get(name)
                .copied()
                .ok_or_else(|| FormulaError::UnknownIdent(name.clone())),
            Node::Neg(inner) => Ok(-inner.eval(vars)?),
            Node::Add(a, b) => Ok(a.eval(vars)? + b.eval(vars)?),
            Node::Sub(a, b) => Ok(a.eval(vars)? - b.eval(vars)?),
            Node::Mul(a, b) => Ok(a.eval(vars)? * b.eval(vars)?),
            Node::Div(a, b) => {
                let denom = b.eval(vars)?;
                if denom == 0.0 {
                    Err(FormulaError::DivisionByZero)
                } else {
                    Ok(a.eval(vars)? / denom)
                }
            }
        }
    }
}

struct Parser<'a> {
    tokens: &'a mut Vec<Token>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn consume(&mut self) -> Option<Token> {
        if self.pos < self.tokens.len() {
            let tok = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(tok)
        } else {
            None
        }
    }

    fn parse_expr(&mut self) -> Result<Node, FormulaError> {
        let mut left = self.parse_term()?;
        loop {
            match self.peek() {
                Some(Token::Plus) => {
                    self.consume();
                    let right = self.parse_term()?;
                    left = Node::Add(Box::new(left), Box::new(right));
                }
                Some(Token::Minus) => {
                    self.consume();
                    let right = self.parse_term()?;
                    left = Node::Sub(Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_term(&mut self) -> Result<Node, FormulaError> {
        let mut left = self.parse_factor()?;
        loop {
            match self.peek() {
                Some(Token::Star) => {
                    self.consume();
                    let right = self.parse_factor()?;
                    left = Node::Mul(Box::new(left), Box::new(right));
                }
                Some(Token::Slash) => {
                    self.consume();
                    let right = self.parse_factor()?;
                    left = Node::Div(Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_factor(&mut self) -> Result<Node, FormulaError> {
        match self.peek() {
            Some(Token::Minus) => {
                self.consume();
                let inner = self.parse_factor()?;
                Ok(Node::Neg(Box::new(inner)))
            }
            Some(Token::Plus) => {
                self.consume();
                self.parse_factor()
            }
            _ => self.parse_atom(),
        }
    }

    fn parse_atom(&mut self) -> Result<Node, FormulaError> {
        match self.consume() {
            Some(Token::Number(n)) => Ok(Node::Lit(n)),
            Some(Token::Ident(name)) => Ok(Node::Var(name)),
            Some(Token::LParen) => {
                let inner = self.parse_expr()?;
                match self.consume() {
                    Some(Token::RParen) => Ok(inner),
                    _ => Err(FormulaError::Parse("missing `)`".to_string())),
                }
            }
            Some(other) => Err(FormulaError::Parse(format!(
                "unexpected token {:?}",
                other
            ))),
            None => Err(FormulaError::Parse("unexpected end of input".to_string())),
        }
    }
}

// ─── JSON param walker ────────────────────────────────────────────

/// Walk a feature-call `params` JSON value and substitute any
/// formula-shaped leaves against the variables table. Returns a
/// fresh `Value` with every formula replaced by its evaluated
/// `f32`. Original leaves that aren't formulas come through
/// unchanged.
///
/// The substitution rules are intentionally narrow:
///
/// * A `{"value": "=…", "unit": "…"}` object (Length/Angle
///   wire form) has its `value` replaced by the evaluated
///   number.
/// * A bare string leaf starting with `=` is replaced by the
///   evaluated number (for future f32 fields that grow
///   formula support).
/// * Everything else — arrays of numbers (vec3), primitive
///   strings without the sigil, nested objects — is traversed
///   recursively but left alone.
pub fn resolve_params_with_variables(
    value: &mut serde_json::Value,
    vars: &BTreeMap<String, f32>,
) -> Result<(), FormulaError> {
    match value {
        serde_json::Value::Object(map) => {
            // Length / Angle shape detection: the object has a
            // `value` string starting with `=` AND a `unit`
            // sibling. Resolve in place.
            let is_length_shaped = map.get("unit").is_some()
                && matches!(
                    map.get("value"),
                    Some(serde_json::Value::String(s)) if s.starts_with('=')
                );
            if is_length_shaped {
                let formula = match map.get("value") {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    _ => unreachable!(),
                };
                let resolved = resolve_formula(&formula, vars)?;
                map.insert(
                    "value".to_string(),
                    serde_json::Value::from(resolved as f64),
                );
                return Ok(());
            }
            // Recurse into every child.
            for (_, child) in map.iter_mut() {
                resolve_params_with_variables(child, vars)?;
            }
            Ok(())
        }
        serde_json::Value::Array(arr) => {
            for child in arr.iter_mut() {
                resolve_params_with_variables(child, vars)?;
            }
            Ok(())
        }
        serde_json::Value::String(s) if s.starts_with('=') => {
            let resolved = resolve_formula(s, vars)?;
            *value = serde_json::Value::from(resolved as f64);
            Ok(())
        }
        _ => Ok(()),
    }
}

// ─── tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn vars_of(entries: &[(&str, f32)]) -> BTreeMap<String, f32> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect()
    }

    #[test]
    fn literal_number_resolves_to_itself() {
        let vars = BTreeMap::new();
        assert_eq!(resolve_formula("=42", &vars).unwrap(), 42.0);
        assert!((resolve_formula("=3.14", &vars).unwrap() - 3.14).abs() < 1e-5);
    }

    #[test]
    fn bare_variable_reference_resolves() {
        let vars = vars_of(&[("width", 50.0), ("depth", 30.0)]);
        assert_eq!(resolve_formula("=width", &vars).unwrap(), 50.0);
        assert_eq!(resolve_formula("= depth ", &vars).unwrap(), 30.0);
    }

    #[test]
    fn arithmetic_honours_precedence() {
        let vars = vars_of(&[("width", 10.0)]);
        assert_eq!(
            resolve_formula("=width / 2 + 5", &vars).unwrap(),
            10.0
        );
        // (width + 5) * 2 = 30
        assert_eq!(
            resolve_formula("=(width + 5) * 2", &vars).unwrap(),
            30.0
        );
        // 2 * -width = -20
        assert_eq!(
            resolve_formula("=2 * -width", &vars).unwrap(),
            -20.0
        );
    }

    #[test]
    fn unknown_identifier_errors() {
        let vars = vars_of(&[("width", 10.0)]);
        match resolve_formula("=height", &vars) {
            Err(FormulaError::UnknownIdent(name)) => assert_eq!(name, "height"),
            other => panic!("expected UnknownIdent, got {other:?}"),
        }
    }

    #[test]
    fn missing_sigil_errors() {
        let vars = BTreeMap::new();
        assert!(matches!(
            resolve_formula("42", &vars),
            Err(FormulaError::MissingSigil)
        ));
    }

    #[test]
    fn division_by_zero_errors() {
        let vars = vars_of(&[("zero", 0.0)]);
        assert!(matches!(
            resolve_formula("=10 / zero", &vars),
            Err(FormulaError::DivisionByZero)
        ));
    }

    #[test]
    fn parens_and_nested_expressions() {
        let vars = vars_of(&[("a", 2.0), ("b", 3.0), ("c", 4.0)]);
        // (a + b) * c - a = 18
        assert_eq!(
            resolve_formula("=(a + b) * c - a", &vars).unwrap(),
            18.0
        );
    }

    #[test]
    fn trailing_garbage_errors() {
        let vars = BTreeMap::new();
        assert!(matches!(
            resolve_formula("=1 + 2 )", &vars),
            Err(FormulaError::Parse(_))
        ));
    }

    // ─── param-walker tests ─────────────────────────────────

    #[test]
    fn walker_substitutes_length_shaped_value() {
        let vars = vars_of(&[("width", 50.0)]);
        let mut v = serde_json::json!({
            "diameter": { "value": "=width / 2", "unit": "mm" },
            "depth": { "value": 40.0, "unit": "mm" },
            "translate": [0.0, 0.0, 0.0],
        });
        resolve_params_with_variables(&mut v, &vars).unwrap();
        assert_eq!(v["diameter"]["value"], serde_json::json!(25.0));
        assert_eq!(v["depth"]["value"], serde_json::json!(40.0));
    }

    #[test]
    fn walker_leaves_plain_numeric_values_alone() {
        let vars = BTreeMap::new();
        let mut v = serde_json::json!({
            "half_extents": [50.0, 50.0, 50.0],
            "op": "union",
            "name": "Box 1",
        });
        let before = v.clone();
        resolve_params_with_variables(&mut v, &vars).unwrap();
        assert_eq!(v, before);
    }

    #[test]
    fn walker_propagates_unknown_identifier_errors() {
        let vars = BTreeMap::new();
        let mut v = serde_json::json!({
            "depth": { "value": "=height", "unit": "mm" },
        });
        match resolve_params_with_variables(&mut v, &vars) {
            Err(FormulaError::UnknownIdent(name)) => assert_eq!(name, "height"),
            other => panic!("expected UnknownIdent, got {other:?}"),
        }
    }
}
