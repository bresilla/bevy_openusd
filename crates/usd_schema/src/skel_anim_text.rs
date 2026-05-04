//! Text-mode USDA scanner that extracts `UsdSkelAnimation` prim data.
//!
//! Walks raw .usda text looking for `def SkelAnimation "name" { ... }`
//! blocks, extracts the `joints` token array plus the time-sampled
//! `translations` (float3[]) / `rotations` (quatf[] or quath[]) /
//! `scales` (float3[] or half3[]) attributes. The values are stored
//! per timecode so the runtime driver can interpolate at any stage
//! time.
//!
//! Why a separate text parser:
//! `openusd-rs` 0.3 errors on `Unsupported property metadata value
//! token: Punctuation('(')` when it sees tuple-valued timeSamples in
//! property values:
//!
//! ```text
//! quatf[] rotations.timeSamples = {
//!     101: [(0.99, -0.07, ...), (0.99, 0.07, ...), ...],
//!     102: [...],
//! }
//! ```
//!
//! The parser bails before our schema readers can see anything. This
//! module sidesteps the issue by reading the text directly. It is
//! intentionally narrow: it only understands SkelAnimation — every
//! other prim type is left to the real composition pipeline.
//!
//! The parser is forgiving: unknown attributes are skipped, missing
//! ones return defaults. Time values are stored as `f64` to match
//! USD's timecode convention.

use std::collections::BTreeMap;

/// One SkelAnimation prim's decoded animation data. `joints` is the
/// per-frame ordering used by every `Vec` in the per-time samples;
/// it does NOT have to match the bound Skeleton's `joints` array
/// (the consumer must remap by name).
#[derive(Debug, Clone, Default)]
pub struct ReadSkelAnimText {
    /// Stringified absolute prim path of the SkelAnimation. Either
    /// the prim's authored name relative to the closest parent in the
    /// text, or the full path the consumer passed in. Filled by
    /// callers that compose the path; the parser sets it to the
    /// prim's leaf name (e.g. `"SkelAnim"`).
    pub prim_name: String,
    /// `joints = ["A", "A/B", ...]` — drives the per-time arrays.
    pub joints: Vec<String>,
    /// `blendShapes = [...]` — per-time `blend_shape_weights` ordering.
    pub blend_shapes: Vec<String>,
    /// `translations.timeSamples = { time: [(x,y,z), ...], ... }`.
    /// Inner Vec length matches `joints.len()` for valid samples.
    pub translations: BTreeMap<OrdF64, Vec<[f32; 3]>>,
    /// `rotations.timeSamples = { time: [(w,x,y,z), ...], ... }`.
    /// Inner Vec length matches `joints.len()` for valid samples.
    /// USD authors quaternions as (real, imaginary) = (w, x, y, z).
    pub rotations: BTreeMap<OrdF64, Vec<[f32; 4]>>,
    /// `scales.timeSamples = { time: [(x,y,z), ...], ... }`.
    /// Inner Vec length matches `joints.len()` for valid samples.
    pub scales: BTreeMap<OrdF64, Vec<[f32; 3]>>,
    /// `blendShapeWeights.timeSamples = { time: [w, w, ...], ... }`.
    /// Inner Vec length matches `blend_shapes.len()`.
    pub blend_shape_weights: BTreeMap<OrdF64, Vec<f32>>,
}

/// Wraps `f64` so it can key a `BTreeMap`. USD timecodes are
/// `f64` per spec but never `NaN` in practice; we panic on `NaN` to
/// surface bad input early.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct OrdF64(pub f64);
impl Eq for OrdF64 {}
impl Ord for OrdF64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .partial_cmp(&other.0)
            .expect("OrdF64: NaN timecode in SkelAnimation samples")
    }
}

/// Scan `text` (the contents of a .usda file) for every
/// `UsdSkelAnimation` prim and return the decoded animation data.
/// Caller is responsible for prefixing the returned `prim_name` with
/// the file's prim-path scope when constructing absolute paths for
/// lookup.
pub fn scan_skel_animations(text: &str) -> Vec<ReadSkelAnimText> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(start) = find_skel_anim_def(text, cursor) {
        // Locate the body block `{ ... }`. We assume the brace
        // structure is well-formed (USD authoring tools emit balanced
        // braces).
        let Some(open) = text[start..].find('{').map(|i| start + i) else {
            break;
        };
        let Some(close) = matched_brace(text, open) else {
            break;
        };
        let header = &text[start..open];
        let body = &text[open + 1..close];

        let prim_name = extract_prim_name(header).unwrap_or_default();
        let mut anim = ReadSkelAnimText {
            prim_name,
            ..Default::default()
        };

        // Find each named property. Order doesn't matter; tools emit
        // them in any order.
        anim.joints = extract_token_array(body, "joints").unwrap_or_default();
        anim.blend_shapes = extract_token_array(body, "blendShapes").unwrap_or_default();

        anim.translations = extract_vec3_timesamples(body, "translations");
        anim.rotations = extract_quat_timesamples(body, "rotations");
        anim.scales = extract_vec3_timesamples(body, "scales");
        anim.blend_shape_weights = extract_scalar_array_timesamples(body, "blendShapeWeights");

        out.push(anim);
        cursor = close + 1;
    }
    out
}

// ── locator helpers ────────────────────────────────────────────────

/// Find the next `def SkelAnimation` keyword starting at `from`.
fn find_skel_anim_def(text: &str, from: usize) -> Option<usize> {
    // Match either `def SkelAnimation` or `over SkelAnimation` so
    // overrides also surface. Whitespace tolerant.
    let mut search = from;
    loop {
        let rest = &text[search..];
        let pos = rest.find("SkelAnimation")?;
        let abs = search + pos;
        // Walk back past whitespace to find the prefix keyword.
        let prefix_end = abs;
        let prefix_start = text[..prefix_end].trim_end().len();
        let prefix = text[..prefix_end].get(prefix_start..)?;
        let _ = prefix;
        // Cheaper: just check the keyword family directly before the match.
        let before = &text[..abs];
        if before.trim_end().ends_with("def") || before.trim_end().ends_with("over") {
            // Make sure we're at a token boundary on the right too.
            let after = &text[abs + "SkelAnimation".len()..];
            if after.starts_with(|c: char| c.is_whitespace() || c == '"' || c == '(') {
                return Some(abs);
            }
        }
        search = abs + "SkelAnimation".len();
    }
}

/// `def SkelAnimation "Name" (...)` → "Name". Header tail starts at
/// the keyword; we just pull out the first quoted string.
fn extract_prim_name(header: &str) -> Option<String> {
    let q1 = header.find('"')?;
    let q2 = header[q1 + 1..].find('"')?;
    Some(header[q1 + 1..q1 + 1 + q2].to_string())
}

/// Find the `}` that matches the `{` at `open`. Skips strings (so a
/// `}` inside a quoted token doesn't close a block) and tracks brace
/// depth.
fn matched_brace(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(open)? != &b'{' {
        return None;
    }
    let mut depth = 0i32;
    let mut i = open;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                in_string = false;
            }
        } else {
            match c {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

// ── attribute extractors ───────────────────────────────────────────

/// Find `<name> = [ ... ]` (token array, default value) or
/// `uniform token[] <name> = [...]`. Returns the unescaped strings.
fn extract_token_array(body: &str, attr: &str) -> Option<Vec<String>> {
    // Match `<attr> = [` after the `=` sign on any line that contains
    // `<attr>` as a property name. We do a lightweight regex-style
    // scan: find the bare attr name, then look for `=` then `[`.
    let mut search = 0usize;
    while let Some(rel) = body[search..].find(attr) {
        let pos = search + rel;
        // Token-boundary check: previous char must be whitespace / `]` / `[`.
        let before_ok = pos == 0 || is_token_boundary(body.as_bytes()[pos - 1] as char);
        let after = pos + attr.len();
        // Next non-whitespace must be `=` or `[` (for `[]` brackets).
        let after_ch = body[after..].chars().next();
        if !before_ok {
            search = after;
            continue;
        }
        if let Some(c) = after_ch {
            if c.is_alphanumeric() || c == '_' || c == ':' {
                search = after;
                continue;
            }
        }
        // Look for `=` after the attr name.
        let Some(eq_rel) = body[after..].find('=') else {
            return None;
        };
        let value_start = after + eq_rel + 1;
        // Skip whitespace.
        let value_text = body[value_start..].trim_start();
        if !value_text.starts_with('[') {
            search = after;
            continue;
        }
        let arr_open = value_start + body[value_start..].find('[').unwrap();
        // Find matching `]`. Skip strings.
        let arr_close = match_bracket(body.as_bytes(), arr_open, b'[', b']')?;
        let inner = &body[arr_open + 1..arr_close];
        return Some(parse_quoted_strings(inner));
    }
    None
}

fn is_token_boundary(c: char) -> bool {
    c.is_whitespace() || c == ';' || c == '}' || c == ')'
}

fn match_bracket(bytes: &[u8], open: usize, openc: u8, closec: u8) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                in_string = false;
            }
        } else {
            if c == b'"' {
                in_string = true;
            } else if c == openc {
                depth += 1;
            } else if c == closec {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

/// Extract every `"..."` substring from `text`, ignoring escapes
/// inside them. Returns the raw inner contents.
fn parse_quoted_strings(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    out.push(
                        std::str::from_utf8(&bytes[start..i])
                            .unwrap_or("")
                            .to_string(),
                    );
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Extract `<attr>.timeSamples = { time: [(x,y,z), ...], ... }`.
fn extract_vec3_timesamples(body: &str, attr: &str) -> BTreeMap<OrdF64, Vec<[f32; 3]>> {
    let mut out = BTreeMap::new();
    let Some((time_block, _)) = locate_timesamples_block(body, attr) else {
        return out;
    };
    for (time, value_text) in iter_time_entries(time_block) {
        let mut samples = Vec::new();
        for tup in iter_tuples(value_text) {
            let parts: Vec<f32> = tup
                .split(',')
                .filter_map(|s| s.trim().parse::<f32>().ok())
                .collect();
            if parts.len() == 3 {
                samples.push([parts[0], parts[1], parts[2]]);
            }
        }
        out.insert(OrdF64(time), samples);
    }
    out
}

/// Extract `<attr>.timeSamples = { time: [(w,x,y,z), ...], ... }`.
fn extract_quat_timesamples(body: &str, attr: &str) -> BTreeMap<OrdF64, Vec<[f32; 4]>> {
    let mut out = BTreeMap::new();
    let Some((time_block, _)) = locate_timesamples_block(body, attr) else {
        return out;
    };
    for (time, value_text) in iter_time_entries(time_block) {
        let mut samples = Vec::new();
        for tup in iter_tuples(value_text) {
            let parts: Vec<f32> = tup
                .split(',')
                .filter_map(|s| s.trim().parse::<f32>().ok())
                .collect();
            if parts.len() == 4 {
                samples.push([parts[0], parts[1], parts[2], parts[3]]);
            }
        }
        out.insert(OrdF64(time), samples);
    }
    out
}

/// Extract `<attr>.timeSamples = { time: [s, s, s, ...], ... }`
/// (flat scalar array, no tuples). Used for `blendShapeWeights`.
fn extract_scalar_array_timesamples(body: &str, attr: &str) -> BTreeMap<OrdF64, Vec<f32>> {
    let mut out = BTreeMap::new();
    let Some((time_block, _)) = locate_timesamples_block(body, attr) else {
        return out;
    };
    for (time, value_text) in iter_time_entries(time_block) {
        // value_text looks like "[s, s, s, ...]" — strip brackets,
        // split by comma.
        let trimmed = value_text.trim().trim_start_matches('[').trim_end_matches(']');
        let samples: Vec<f32> = trimmed
            .split(',')
            .filter_map(|s| s.trim().parse::<f32>().ok())
            .collect();
        out.insert(OrdF64(time), samples);
    }
    out
}

/// Locate the `<attr>.timeSamples = { ... }` block. Returns the
/// inner text between the braces.
fn locate_timesamples_block<'a>(body: &'a str, attr: &str) -> Option<(&'a str, usize)> {
    let needle = format!("{attr}.timeSamples");
    let pos = body.find(&needle)?;
    let after = pos + needle.len();
    let eq_rel = body[after..].find('=')?;
    let value_start = after + eq_rel + 1;
    let brace_rel = body[value_start..].find('{')?;
    let open = value_start + brace_rel;
    let close = matched_brace(body, open)?;
    Some((&body[open + 1..close], close))
}

/// Iterate `time: <value>` entries inside a timeSamples block. The
/// value may be an array `[...]` containing tuples, or a scalar/flat
/// array. Yields the raw value text (without leading whitespace) for
/// the caller to parse.
fn iter_time_entries(block: &str) -> Vec<(f64, &str)> {
    let mut out = Vec::new();
    let bytes = block.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip whitespace + commas.
        while i < bytes.len()
            && (bytes[i].is_ascii_whitespace() || bytes[i] == b',')
        {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Parse number until ':'.
        let num_start = i;
        while i < bytes.len() && bytes[i] != b':' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let num_text = std::str::from_utf8(&bytes[num_start..i])
            .unwrap_or("")
            .trim();
        let Ok(time) = num_text.parse::<f64>() else {
            // Not a real time entry; skip to next comma.
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            continue;
        };
        i += 1; // past ':'.
        // Find the value extent. We support `[ ... ]` arrays
        // (matching brackets) and flat numbers terminated by `,` or
        // end-of-block.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let value_start = i;
        if i < bytes.len() && bytes[i] == b'[' {
            let value_end = match_bracket(bytes, i, b'[', b']');
            if let Some(end) = value_end {
                let value = std::str::from_utf8(&bytes[value_start..=end]).unwrap_or("");
                out.push((time, value));
                i = end + 1;
            } else {
                break;
            }
        } else {
            // Scalar value — read until comma or end.
            while i < bytes.len() && bytes[i] != b',' && bytes[i] != b'}' {
                i += 1;
            }
            let value = std::str::from_utf8(&bytes[value_start..i]).unwrap_or("");
            out.push((time, value));
        }
    }
    out
}

/// Iterate `(...)` tuples inside an array value text. Yields the
/// inner contents (without parens), e.g. `"0.99, -0.07, 0, 0.04"`.
fn iter_tuples(value_text: &str) -> Vec<&str> {
    let bytes = value_text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            let start = i + 1;
            // Find matching `)`.
            let mut depth = 1i32;
            i += 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            let inner = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                            out.push(inner);
                            i += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_skel_animation() {
        let text = r#"
            def SkelAnimation "SkelAnim"
            {
                uniform token[] joints = ["A", "A/B"]
                quatf[] rotations.timeSamples = {
                    101: [(1, 0, 0, 0), (0.7, 0.7, 0, 0)],
                    102: [(0.5, 0.5, 0.5, 0.5), (1, 0, 0, 0)],
                }
                float3[] translations.timeSamples = {
                    101: [(0, 0, 0), (1, 2, 3)],
                }
            }
        "#;
        let anims = scan_skel_animations(text);
        assert_eq!(anims.len(), 1);
        let a = &anims[0];
        assert_eq!(a.prim_name, "SkelAnim");
        assert_eq!(a.joints, vec!["A".to_string(), "A/B".to_string()]);
        assert_eq!(a.rotations.len(), 2);
        assert_eq!(a.rotations[&OrdF64(101.0)][0], [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(a.rotations[&OrdF64(101.0)][1], [0.7, 0.7, 0.0, 0.0]);
        assert_eq!(a.rotations[&OrdF64(102.0)][0], [0.5, 0.5, 0.5, 0.5]);
        assert_eq!(a.translations.len(), 1);
        assert_eq!(a.translations[&OrdF64(101.0)][1], [1.0, 2.0, 3.0]);
        assert_eq!(a.scales.len(), 0);
    }

    #[test]
    fn skips_unrelated_prims() {
        let text = r#"
            def Mesh "Body" {}
            def Skeleton "Skel" {
                uniform token[] joints = ["X"]
            }
            def SkelAnimation "Anim"
            {
                uniform token[] joints = ["X"]
                float3[] translations.timeSamples = { 0: [(1, 2, 3)] }
            }
        "#;
        let anims = scan_skel_animations(text);
        assert_eq!(anims.len(), 1);
        assert_eq!(anims[0].joints, vec!["X".to_string()]);
        assert_eq!(anims[0].translations[&OrdF64(0.0)][0], [1.0, 2.0, 3.0]);
    }

    #[test]
    fn parses_blendshape_weights() {
        let text = r#"
            def SkelAnimation "Anim"
            {
                uniform token[] blendShapes = ["bs0", "bs1"]
                float[] blendShapeWeights.timeSamples = {
                    0: [0.0, 0.5],
                    1: [1.0, 0.25],
                }
            }
        "#;
        let anims = scan_skel_animations(text);
        assert_eq!(anims.len(), 1);
        assert_eq!(anims[0].blend_shapes.len(), 2);
        assert_eq!(anims[0].blend_shape_weights[&OrdF64(0.0)], vec![0.0, 0.5]);
        assert_eq!(anims[0].blend_shape_weights[&OrdF64(1.0)], vec![1.0, 0.25]);
    }
}
