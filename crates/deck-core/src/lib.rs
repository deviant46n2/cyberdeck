//! deck-core: inventory scanner, format parsers, fit estimator, dedup.
//! Boring internals on purpose — theming lives in the frontend only.

pub mod dedup;
pub mod fit;
pub mod gguf;
pub mod importer;
pub mod model;
pub mod profile;
pub mod safetensors;
pub mod scanner;
pub mod store;
