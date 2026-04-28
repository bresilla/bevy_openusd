//! Strip Omniverse-only USDA prim metadata that openusd-rs's text
//! parser doesn't yet understand.
//!
//! Pixar's Sdf grammar accepts arbitrary string tokens in the prim
//! metadata block (`( ... )` after the prim header). Omniverse
//! authors `hide_in_stage_window = false` and `no_delete = true`
//! routinely; openusd-rs rejects them with
//! `Unsupported metadata: <key>`. Stripping these BEFORE the parser
//! sees them — and replacing each line with a USDA comment so diffs
//! stay obvious — is the cheapest workaround until upstream learns
//! to ignore unknown tokens.

const UNSUPPORTED_USDA_PRIM_METADATA: &[&str] = &["hide_in_stage_window", "no_delete"];

/// Rewrite `bytes` (assumed UTF-8 USDA) replacing each
/// known-unsupported prim metadata assignment with a USDA comment
/// carrying the original text. Operates line-by-line:
/// - first non-whitespace token must match a key in the list, AND
/// - the rest of the line must look like `= <scalar>` terminated
///   by newline (no `[`, `{`, or `(` openings — those are
///   structured values the parser may need).
///
/// Multi-line values and list ops are left alone (none of the
/// known-unsupported keys appear in those forms in practice).
/// Returns the input unchanged when it isn't valid UTF-8.
pub fn strip_unsupported_prim_metadata(bytes: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return bytes.to_vec();
    };
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(key) = UNSUPPORTED_USDA_PRIM_METADATA
            .iter()
            .find(|k| trimmed.starts_with(*k))
            && looks_like_scalar_assignment(&trimmed[key.len()..])
        {
            // Keep the original indentation so column offsets of subsequent
            // lines (and the `(...)` closing paren) stay put.
            let indent: String = line
                .chars()
                .take_while(|c| c.is_whitespace() && *c != '\n')
                .collect();
            out.push_str(&indent);
            out.push_str("# [usd_schemas::3rd_party] stripped: ");
            out.push_str(trimmed.trim_end_matches(['\r', '\n']));
            out.push('\n');
            continue;
        }
        out.push_str(line);
    }
    out.into_bytes()
}

fn looks_like_scalar_assignment(rest: &str) -> bool {
    let r = rest.trim_start();
    if !r.starts_with('=') {
        return false;
    }
    let rhs = r[1..].trim_start();
    let end = rhs.find(['\r', '\n']).unwrap_or(rhs.len());
    let rhs = rhs[..end].trim_end();
    !rhs.is_empty() && !rhs.starts_with(['[', '{', '('])
}
