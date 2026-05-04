//! Resolver that runs the USDA metadata-strip preprocess on **every**
//! `.usda` asset openusd opens — sublayers, references, payloads, not
//! just the root layer.
//!
//! When a downstream loader strips only the root-layer bytes it hands
//! to `Stage::open`, the parser still calls back into the resolver's
//! `open_asset` to fetch each composition arc — bypassing that root-only
//! preprocess. Omniverse scenes that author `hide_in_stage_window` /
//! `no_delete` on nested prims would still trip the parser.
//!
//! This wrapper delegates every method to `DefaultResolver` except
//! `open_asset` — which reads the raw bytes, runs them through
//! [`super::strip_metadata::strip_unsupported_prim_metadata`] when the
//! asset is a `.usda`, and returns the mutated bytes as an `io::Cursor`
//! (openusd's `Asset` trait is `Read + Seek + Send`; `Cursor<Vec<u8>>`
//! satisfies all three).

use std::io;
use std::path::{Path, PathBuf};

use openusd::ar::{Asset, AssetInfo, DefaultResolver, ResolvedPath, Resolver, ResolverContext};

use super::strip_metadata::strip_unsupported_prim_metadata;

pub struct StripMetadataResolver {
    inner: DefaultResolver,
    /// Same dirs we hand the inner resolver, kept for the path-rewrite
    /// fallback below.
    search_paths: Vec<PathBuf>,
}

impl StripMetadataResolver {
    pub fn with_search_paths(paths: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        let search_paths: Vec<PathBuf> = paths.into_iter().map(|p| p.into()).collect();
        Self {
            inner: DefaultResolver::with_search_paths(search_paths.clone()),
            search_paths,
        }
    }

    /// When a hard-coded absolute path like
    /// `/home/arjan/isaacsim-greenhouse/materials/Clear_Glass.usd` doesn't
    /// exist on this machine, try to relocate it. Strategy: walk from the
    /// path's tail up, looking for a directory name that also appears in
    /// any of our search paths; if a match exists, splice the local
    /// prefix in for the foreign one. This rescues USD files authored
    /// with absolute paths from another user's home directory — common
    /// in Omniverse / Isaac scenes that were never run through Pixar's
    /// asset-resolver round-trip.
    fn relocate_absolute(&self, asset_path: &str) -> Option<PathBuf> {
        let foreign = Path::new(asset_path);
        if !foreign.is_absolute() {
            return None;
        }
        let comps: Vec<&std::ffi::OsStr> =
            foreign.components().filter_map(|c| match c {
                std::path::Component::Normal(n) => Some(n),
                _ => None,
            }).collect();
        // Walk from the project root forward, looking for the longest
        // suffix of `foreign` that, joined onto one of our search paths
        // (or a parent of one), points at an existing file.
        for split in 0..comps.len() {
            let suffix: PathBuf = comps[split..].iter().collect();
            for root in &self.search_paths {
                let mut cur = root.clone();
                for _ in 0..6 {
                    let candidate = cur.join(&suffix);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                    let Some(parent) = cur.parent() else { break };
                    cur = parent.to_path_buf();
                }
            }
        }
        None
    }
}

impl Resolver for StripMetadataResolver {
    fn create_identifier(&self, asset_path: &str, anchor: Option<&ResolvedPath>) -> String {
        self.inner.create_identifier(asset_path, anchor)
    }

    fn resolve(&self, asset_path: &str) -> Option<ResolvedPath> {
        if let Some(rp) = self.inner.resolve(asset_path) {
            return Some(rp);
        }
        // Inner resolver gave up. If this is an absolute path on a
        // *different* machine, try to relocate it under our search
        // roots before declaring it unresolvable.
        let relocated = self.relocate_absolute(asset_path)?;
        let s = relocated.to_string_lossy();
        eprintln!("resolver: relocated {asset_path:?} → {s:?}");
        self.inner.resolve(&s)
    }

    fn resolve_for_new_asset(&self, asset_path: &str) -> Option<ResolvedPath> {
        self.inner.resolve_for_new_asset(asset_path)
    }

    fn open_asset(&self, resolved_path: &ResolvedPath) -> io::Result<Box<dyn Asset>> {
        // Defer to DefaultResolver to do the actual file read + any
        // package-relative heroics. We only intercept if the bytes we
        // got look like USDA and need stripping.
        let mut inner_asset = self.inner.open_asset(resolved_path)?;
        let ext = resolved_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let is_usda = ext == "usda"
            || (ext == "usd" && {
                // Sniff the first line; `.usd` with no magic bytes = text USDA.
                let mut buf = [0u8; 8];
                use std::io::{Read, Seek, SeekFrom};
                let _ = inner_asset.seek(SeekFrom::Start(0));
                let n = inner_asset.read(&mut buf).unwrap_or(0);
                let _ = inner_asset.seek(SeekFrom::Start(0));
                buf[..n].starts_with(b"#usda")
            });
        if !is_usda {
            return Ok(inner_asset);
        }

        let raw = inner_asset.read_all()?;
        let stripped = strip_unsupported_prim_metadata(&raw);
        Ok(Box::new(io::Cursor::new(stripped)))
    }

    fn get_asset_info(&self, asset_path: &str, resolved_path: &ResolvedPath) -> AssetInfo {
        self.inner.get_asset_info(asset_path, resolved_path)
    }

    fn get_modification_timestamp(
        &self,
        asset_path: &str,
        resolved_path: &ResolvedPath,
    ) -> Option<std::time::SystemTime> {
        self.inner
            .get_modification_timestamp(asset_path, resolved_path)
    }

    fn is_context_dependent_path(&self, asset_path: &str) -> bool {
        self.inner.is_context_dependent_path(asset_path)
    }

    fn create_default_context(&self) -> ResolverContext {
        self.inner.create_default_context()
    }

    fn create_default_context_for_asset(&self, asset_path: &str) -> ResolverContext {
        self.inner.create_default_context_for_asset(asset_path)
    }
}
