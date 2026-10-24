//! Compatibility surface: legacy `api::` paths. These modules re-export the
//! framework-agnostic `crate::types` under the same names; new hosts should
//! use `types` directly.

pub mod node;
pub mod wallet;
