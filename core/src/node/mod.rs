#[cfg(all(not(test), not(target_arch = "wasm32")))]
mod fnn;
#[cfg(any(test, target_arch = "wasm32"))]
mod simulated;

#[cfg(all(not(test), not(target_arch = "wasm32")))]
pub use fnn::NodeMachine;
#[cfg(any(test, target_arch = "wasm32"))]
pub use simulated::NodeMachine;
