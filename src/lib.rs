pub mod engine;
pub mod geom;
pub mod greedy;
pub mod lattice;
pub mod output;
pub mod par;
pub mod svgio;

#[cfg(target_arch = "wasm32")]
mod wasm_api;
