//! Exposes the semantics of the views of a Flutter engine to assistive
//! technologies, such as screen readers, through
//! [AccessKit](https://accesskit.dev).

pub mod bridge;
pub mod tree;

pub use bridge::Accessibility;
