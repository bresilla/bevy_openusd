//! Third-party-glue: workarounds and helpers for the upstream
//! `openusd-rs` crate that don't fit the schema-authoring story
//! `usd_schemas` is otherwise about.
//!
//! - [`strip_metadata`] — strip Omniverse-only USDA prim metadata
//!   (`hide_in_stage_window`, `no_delete`) the upstream parser
//!   chokes on.
//! - [`resolver`] — a `Resolver` shim that runs every USDA asset
//!   through the strip pass before openusd parses it.
//! - [`convert`] — author a `*.preview.usda` override layer that
//!   replaces MDL/OmniPBR materials with `UsdPreviewSurface`
//!   fallbacks so MDL-only stages render through the
//!   pure-OpenUSD shading pipeline.

pub mod convert;
pub mod resolver;
pub mod strip_metadata;
