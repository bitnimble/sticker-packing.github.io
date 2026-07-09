pub mod engine;
pub mod geom;
pub mod greedy;
pub mod lattice;
pub mod output;
pub mod par;
pub mod svgio;

#[cfg(target_arch = "wasm32")]
mod wasm_api;

// Exposes `initThreadPool(n)` to JS: spins up a rayon worker pool over Web Workers.
#[cfg(all(target_arch = "wasm32", feature = "wasm-threads"))]
pub use wasm_bindgen_rayon::init_thread_pool;
