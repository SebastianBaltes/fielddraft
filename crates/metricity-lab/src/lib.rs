//! metricity-lab — gemeinsames Experiment-Harness des Forschungsprogramms
//! "SDF-Distanzerhaltung unter Modellieroperationen"
//! (`docs/research/sdf-metricity/README.md`).
//!
//! Das Crate ist die Messbank, gegen die alle Forschungsagenten ihre
//! Feld-Varianten fahren:
//!
//! * [`ground_truth`] — Brute-Force-Referenzdistanz zur exakten Nullmenge
//!   eines beliebigen Feldes (Oberflächen-Sampling + k-d-Tree +
//!   Richtungs-Raffinierung). Validiert gegen analytische Fälle in
//!   `tests/ground_truth_validation.rs`.
//! * [`metrics`] — Distanzfehler nach Bändern (Nah-/Mittel-/Fernfeld,
//!   innen/außen), Gradientennorm-Statistik, Naht-Sprünge, Kosten.
//! * [`scenes`] — adversarialer Testkorpus (dünne Wände, Fast-Tangenten,
//!   tiefe CSG-Bäume, smin-Ketten, Twist/Bend/Gyroid, MaskedBlend).
//! * [`runner`] — kompletter Messlauf einer Szene inkl. CSV/JSON-Ausgabe.
//!
//! Anleitung für Forschungsagenten: `docs/research/sdf-metricity/HARNESS.md`.

pub mod field;
pub mod ground_truth;
pub mod metrics;
pub mod report;
pub mod runner;
pub mod scenes;
pub mod util;

pub use field::{CsgField, Field, FnField, TapeField};
pub use ground_truth::{GroundTruth, GtOptions};
