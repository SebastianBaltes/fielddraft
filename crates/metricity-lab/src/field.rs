//! Feld-Abstraktion der Messbank.
//!
//! Alles, was die Messbank misst, ist ein [`Field`]: eine Funktion
//! `p ∈ ℝ³ → f(p) ∈ ℝ` mit der Konvention `f < 0` innen, `f > 0` außen,
//! Nullmenge = Oberfläche. Die Kernel-Felder rechnen in `f32`; die
//! Messbank arbeitet an der `f64`-Schnittstelle, damit Ground Truth und
//! Metriken nicht selbst Rundungsfehler einschleppen.

use fielddraft_kernel::csg::{CsgNode, EvalMode};
use fielddraft_kernel::sdf_compiler::{eval_opcodes_with_volumes, CompiledSdf};

/// Ein skalares Implicit-Feld über ℝ³. `Sync`, damit die Messbank
/// Punktmengen mit rayon parallel auswerten kann.
pub trait Field: Sync {
    fn eval(&self, p: [f64; 3]) -> f64;
}

/// Das IST-Feld des Kernels: direkter `CsgNode::sdf`-Eval in einem
/// [`EvalMode`]. `EvalMode::Visual` ist der Mesher-/Logical-Pfad aus der
/// Programm-Charta (unnormalisiertes smin!), `EvalMode::Logical` der
/// harte min/max-Baum.
pub struct CsgField<'a> {
    pub node: &'a CsgNode,
    pub mode: EvalMode,
}

impl<'a> CsgField<'a> {
    pub fn visual(node: &'a CsgNode) -> Self {
        Self { node, mode: EvalMode::Visual }
    }
}

impl Field for CsgField<'_> {
    fn eval(&self, p: [f64; 3]) -> f64 {
        self.node.sdf([p[0] as f32, p[1] as f32, p[2] as f32], self.mode) as f64
    }
}

/// Das kompilierte Bytecode-Tape ([`CompiledSdf`], CPU-Referenz
/// `eval_opcodes`) — der Render-Pfad. Weicht bei MaskedBlend vom
/// `CsgField`-Visual-Feld ab (#382-Render-Fade).
pub struct TapeField<'a> {
    pub compiled: &'a CompiledSdf,
}

impl Field for TapeField<'_> {
    fn eval(&self, p: [f64; 3]) -> f64 {
        eval_opcodes_with_volumes(
            &self.compiled.opcodes,
            [p[0] as f32, p[1] as f32, p[2] as f32],
            &self.compiled.volumes,
        ) as f64
    }
}

/// Beliebige Feld-Variante eines Forschungsagenten als Closure.
///
/// ```ignore
/// let variant = FnField(|p: [f64; 3]| my_corrected_sdf(p));
/// let report = metrics::distance_error_report(&gt, &variant, &points);
/// ```
pub struct FnField<F: Fn([f64; 3]) -> f64 + Sync>(pub F);

impl<F: Fn([f64; 3]) -> f64 + Sync> Field for FnField<F> {
    fn eval(&self, p: [f64; 3]) -> f64 {
        (self.0)(p)
    }
}
