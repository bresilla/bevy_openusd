//! Pack a directory of USD-asset files into a USDZ archive using
//! openusd-rs's built-in `usdz::ArchiveWriter` (64-byte aligned,
//! STORE-only — matches the published Pixar USDZ spec).
//!
//! Usage:
//!     cargo run --release -p bevy_openusd --example pack_usdz -- <root.usd> [output.usdz]
//!
//! Walks the directory containing `<root.usd>`, packs every file
//! through `ArchiveWriter::add_layer` with the root layer first
//! (per the USDZ spec).

use std::path::{Path, PathBuf};

use openusd::usdz::ArchiveWriter;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: pack_usdz <root.usd> [output.usdz]");
        std::process::exit(2);
    }
    let root = PathBuf::from(&args[1]).canonicalize()?;
    let output = match args.get(2) {
        Some(s) => PathBuf::from(s),
        None => {
            let stem = root
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("packed");
            let parent = root
                .parent()
                .and_then(|p| p.parent())
                .unwrap_or_else(|| Path::new("."));
            parent.join(format!("{stem}.usdz"))
        }
    };

    let dir = root
        .parent()
        .ok_or_else(|| anyhow::anyhow!("root has no parent dir"))?;
    let root_rel = root.strip_prefix(dir)?.to_path_buf();

    // Collect every file under `dir`, then sort + reorder so the
    // root layer is first (USDZ spec — Apple Quick Look refuses
    // archives where the first entry isn't a USD layer).
    let mut files: Vec<PathBuf> = Vec::new();
    walk(dir, &mut files)?;
    files.sort();
    if let Some(pos) = files
        .iter()
        .position(|p| p.strip_prefix(dir).map(|r| r == root_rel).unwrap_or(false))
    {
        let chosen = files.remove(pos);
        files.insert(0, chosen);
    }

    eprintln!(
        "pack_usdz: {} → {} ({} entries)",
        root.display(),
        output.display(),
        files.len()
    );

    let mut archive = ArchiveWriter::create(&output)?;
    for path in &files {
        let rel = path.strip_prefix(dir)?;
        let name = rel
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("non-UTF-8 path: {rel:?}"))?
            .replace('\\', "/");
        let bytes = std::fs::read(path)?;
        archive.add_layer(&name, &bytes)?;
    }
    archive.finish()?;

    let written = std::fs::metadata(&output)?.len();
    eprintln!("pack_usdz: wrote {} ({} bytes)", output.display(), written);
    Ok(())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            // Skip dotfiles + temp scratch we author ourselves.
            if name.starts_with('.') {
                continue;
            }
            if entry.file_type()?.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    Ok(())
}
