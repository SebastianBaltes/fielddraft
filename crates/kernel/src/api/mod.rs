//! wasm-bindgen-API des [`Kernel`](crate::Kernel), thematisch in
//! mehrere `#[wasm_bindgen] impl Kernel`-Blöcke aufgeteilt (ein Modul
//! pro API-Familie). wasm-bindgen führt alle Blöcke in derselben
//! JS-Klasse `Kernel` zusammen.

mod bodies_mesh;
mod brep;
mod document;
mod export;
mod features;
mod lichen;
mod gizmo;
mod queries_pick;
mod sketch;
mod variables;
