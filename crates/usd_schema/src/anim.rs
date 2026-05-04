//! Time-sampled attribute helpers.
//!
//! `openusd` surfaces authored time samples as `Value::TimeSamples(Vec<(f64,
//! Value)>)` on the `"timeSamples"` field of an attribute spec. This module
//! evaluates those sample lists at a query time with linear interpolation
//! and hold-at-ends behaviour — exactly matching the default USD
//! interpolation mode.
//!
//! Only the types that show up in `xformOp:*` attribute samples are
//! implemented today: vec3 (translate/scale/rotateXYZ) and quaternion
//! (orient). The `vec3` path accepts `Value::Vec3f`/`Vec3d`/`Float3` and
//! returns `[f32; 3]`.

use openusd::sdf::{Path, Value};

/// Sample list as authored: ordered `(timeCode, value)` pairs. Always
/// sorted by timeCode ascending — openusd guarantees this.
pub type Samples = Vec<(f64, Value)>;

/// Per-attribute sample interpolation. USD's canonical behaviour is
/// stage-wide (`UsdStage::GetInterpolationType`), but in practice
/// tools author `interpolation = "held"` or `"linear"` as metadata on
/// the timeSampled attribute spec when they want per-attribute
/// control. We respect both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterpMode {
    #[default]
    Linear,
    Held,
}

impl InterpMode {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "linear" => Some(Self::Linear),
            "held" => Some(Self::Held),
            _ => None,
        }
    }
}

/// Read the `interpolation` metadata authored on a time-sampled
/// attribute. Defaults to `Linear` when unauthored.
pub fn read_interp_mode(
    stage: &openusd::Stage,
    prim: &Path,
    prop: &str,
) -> anyhow::Result<InterpMode> {
    let attr_path = prim.append_property(prop).map_err(anyhow::Error::from)?;
    let raw = stage
        .field::<Value>(attr_path, "interpolation")
        .map_err(anyhow::Error::from)?;
    if let Some(v) = raw {
        if let Value::Token(s) | Value::String(s) = v
            && let Some(m) = InterpMode::parse(&s)
        {
            return Ok(m);
        }
    }
    Ok(InterpMode::Linear)
}

/// Concrete vec3 sample list, preconverted at load time so runtime code
/// never sees the raw `Value` enum.
pub type Vec3Samples = Vec<(f64, [f32; 3])>;

/// Concrete scalar sample list — used for single-axis rotates and any
/// float/double-typed animated attribute.
pub type ScalarSamples = Vec<(f64, f32)>;

/// A preconverted sample list paired with its authored interpolation
/// mode — the runtime evaluator reads both to decide between linear
/// blend and step-function hold.
#[derive(Debug, Clone)]
pub struct Vec3Track {
    pub samples: Vec3Samples,
    pub mode: InterpMode,
}

#[derive(Debug, Clone)]
pub struct ScalarTrack {
    pub samples: ScalarSamples,
    pub mode: InterpMode,
}

/// Per-prim record of which xform ops carry timeSamples, plus those
/// samples converted to concrete values ready for per-frame evaluation.
/// Ops NOT listed here fall back to the prim's default-at-load
/// Transform.
///
/// Vec3 xform ops (`translate`, `rotateXYZ`, `scale`) need openusd's
/// USDA parser to accept tuple-valued timeSamples — a fix that isn't
/// in the pinned commit yet, so authoring those via `.usda` is blocked
/// today. Scalar single-axis rotates (`rotateY`, etc.) work right now.
#[derive(Debug, Clone, Default)]
pub struct AnimatedPrim {
    pub translate: Option<Vec3Track>,
    pub rotate_xyz: Option<Vec3Track>,
    pub scale: Option<Vec3Track>,
    /// Single-axis rotate samples, Euler degrees. One slot per axis —
    /// only one is expected to be non-empty per prim (the one named in
    /// `xformOpOrder`).
    pub rotate_x: Option<ScalarTrack>,
    pub rotate_y: Option<ScalarTrack>,
    pub rotate_z: Option<ScalarTrack>,
}

impl AnimatedPrim {
    pub fn is_empty(&self) -> bool {
        self.translate.is_none()
            && self.rotate_xyz.is_none()
            && self.scale.is_none()
            && self.rotate_x.is_none()
            && self.rotate_y.is_none()
            && self.rotate_z.is_none()
    }
}

/// Read any time-sampled xformOp on `prim` and pre-convert the samples,
/// including the authored `interpolation` metadata. Returns `None`
/// when no op authors timeSamples.
pub fn read_animated_prim(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<AnimatedPrim>> {
    fn vec3_track(
        stage: &openusd::Stage,
        prim: &Path,
        prop: &str,
    ) -> anyhow::Result<Option<Vec3Track>> {
        let Some(samples) = read_samples(stage, prim, prop)? else {
            return Ok(None);
        };
        Ok(Some(Vec3Track {
            samples: samples_to_vec3(samples),
            mode: read_interp_mode(stage, prim, prop)?,
        }))
    }
    fn scalar_track(
        stage: &openusd::Stage,
        prim: &Path,
        prop: &str,
    ) -> anyhow::Result<Option<ScalarTrack>> {
        let Some(samples) = read_samples(stage, prim, prop)? else {
            return Ok(None);
        };
        Ok(Some(ScalarTrack {
            samples: samples_to_scalar(samples),
            mode: read_interp_mode(stage, prim, prop)?,
        }))
    }

    let translate = vec3_track(stage, prim, "xformOp:translate")?;
    let rotate_xyz = vec3_track(stage, prim, "xformOp:rotateXYZ")?;
    let scale = vec3_track(stage, prim, "xformOp:scale")?;
    let rotate_x = scalar_track(stage, prim, "xformOp:rotateX")?;
    let rotate_y = scalar_track(stage, prim, "xformOp:rotateY")?;
    let rotate_z = scalar_track(stage, prim, "xformOp:rotateZ")?;
    let out = AnimatedPrim {
        translate,
        rotate_xyz,
        scale,
        rotate_x,
        rotate_y,
        rotate_z,
    };
    Ok((!out.is_empty()).then_some(out))
}

fn samples_to_vec3(samples: Samples) -> Vec3Samples {
    samples
        .into_iter()
        .filter_map(|(t, v)| value_to_vec3f(&v).map(|a| (t, a)))
        .collect()
}

fn samples_to_scalar(samples: Samples) -> ScalarSamples {
    samples
        .into_iter()
        .filter_map(|(t, v)| value_to_scalar_f32(&v).map(|a| (t, a)))
        .collect()
}

fn value_to_scalar_f32(v: &Value) -> Option<f32> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Double(d) => Some(*d as f32),
        Value::Int(i) => Some(*i as f32),
        Value::Int64(i) => Some(*i as f32),
        _ => None,
    }
}

/// Sample a pre-converted scalar sample list at `t`. Linear between
/// neighbours, hold at ends.
pub fn sample_scalar_concrete(samples: &[(f64, f32)], t: f64) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    let t_first = samples.first().unwrap().0;
    let t_last = samples.last().unwrap().0;
    if t <= t_first {
        return Some(samples.first().unwrap().1);
    }
    if t >= t_last {
        return Some(samples.last().unwrap().1);
    }
    let idx = samples
        .binary_search_by(|(tt, _)| tt.partial_cmp(&t).unwrap_or(std::cmp::Ordering::Equal));
    let (lo, hi) = match idx {
        Ok(i) => return Some(samples[i].1),
        Err(i) => (i - 1, i),
    };
    let (tl, vl) = &samples[lo];
    let (th, vh) = &samples[hi];
    let u = ((t - tl) / (th - tl)) as f32;
    Some(vl + (vh - vl) * u)
}

/// Held sampler: returns the value of the sample at or just BEFORE `t`
/// with no blending (step function). Matches USD's `interpolation =
/// "held"` behaviour: each value stays active until the next sample's
/// timeCode.
pub fn sample_vec3_held(samples: &[(f64, [f32; 3])], t: f64) -> Option<[f32; 3]> {
    if samples.is_empty() {
        return None;
    }
    let t_first = samples.first().unwrap().0;
    if t < t_first {
        return Some(samples.first().unwrap().1);
    }
    // Find the last sample with time <= t.
    let mut chosen = samples.first().unwrap().1;
    for (tt, v) in samples {
        if *tt <= t {
            chosen = *v;
        } else {
            break;
        }
    }
    Some(chosen)
}

/// Scalar counterpart to `sample_vec3_held`.
pub fn sample_scalar_held(samples: &[(f64, f32)], t: f64) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    let t_first = samples.first().unwrap().0;
    if t < t_first {
        return Some(samples.first().unwrap().1);
    }
    let mut chosen = samples.first().unwrap().1;
    for (tt, v) in samples {
        if *tt <= t {
            chosen = *v;
        } else {
            break;
        }
    }
    Some(chosen)
}

/// Convenience: dispatch on `Vec3Track::mode` to pick the right
/// sampler. Falls back to `None` when the track is empty.
pub fn eval_vec3_track(track: &Vec3Track, t: f64) -> Option<[f32; 3]> {
    match track.mode {
        InterpMode::Linear => sample_vec3_concrete(&track.samples, t),
        InterpMode::Held => sample_vec3_held(&track.samples, t),
    }
}

/// Convenience: dispatch on `ScalarTrack::mode`.
pub fn eval_scalar_track(track: &ScalarTrack, t: f64) -> Option<f32> {
    match track.mode {
        InterpMode::Linear => sample_scalar_concrete(&track.samples, t),
        InterpMode::Held => sample_scalar_held(&track.samples, t),
    }
}

/// Sample a pre-converted vec3 sample list at `t`. Linear between
/// neighbours, hold at ends.
pub fn sample_vec3_concrete(samples: &[(f64, [f32; 3])], t: f64) -> Option<[f32; 3]> {
    if samples.is_empty() {
        return None;
    }
    let t_first = samples.first().unwrap().0;
    let t_last = samples.last().unwrap().0;
    if t <= t_first {
        return Some(samples.first().unwrap().1);
    }
    if t >= t_last {
        return Some(samples.last().unwrap().1);
    }
    let idx = samples
        .binary_search_by(|(tt, _)| tt.partial_cmp(&t).unwrap_or(std::cmp::Ordering::Equal));
    let (lo, hi) = match idx {
        Ok(i) => return Some(samples[i].1),
        Err(i) => (i - 1, i),
    };
    let (tl, vl) = &samples[lo];
    let (th, vh) = &samples[hi];
    let u = ((t - tl) / (th - tl)) as f32;
    Some([
        vl[0] + (vh[0] - vl[0]) * u,
        vl[1] + (vh[1] - vl[1]) * u,
        vl[2] + (vh[2] - vl[2]) * u,
    ])
}

/// Read the raw timeSamples from `prim.<prop>`. `Ok(None)` when the
/// attribute doesn't author timeSamples (either because it's a plain
/// default-only attribute or because it doesn't exist).
pub fn read_samples(
    stage: &openusd::Stage,
    prim: &Path,
    prop: &str,
) -> anyhow::Result<Option<Samples>> {
    let attr_path = prim.append_property(prop).map_err(anyhow::Error::from)?;
    let raw = stage
        .field::<Value>(attr_path, "timeSamples")
        .map_err(anyhow::Error::from)?;
    Ok(match raw {
        Some(Value::TimeSamples(v)) => Some(v),
        _ => None,
    })
}

/// Evaluate a vec3 sample list at `t`. Linear interpolation between
/// neighbours; hold-first / hold-last past the endpoints. Returns
/// `None` if the sample list is empty or contains only non-vec3
/// values.
pub fn sample_vec3_at(samples: &[(f64, Value)], t: f64) -> Option<[f32; 3]> {
    if samples.is_empty() {
        return None;
    }
    let t_first = samples.first().unwrap().0;
    let t_last = samples.last().unwrap().0;
    if t <= t_first {
        return value_to_vec3f(&samples.first().unwrap().1);
    }
    if t >= t_last {
        return value_to_vec3f(&samples.last().unwrap().1);
    }
    // Binary search for the bracketing pair. Both arms of the bracket
    // must decode to a vec3 or we fail the whole lookup rather than
    // silently jumping.
    let idx = samples
        .binary_search_by(|(tt, _)| tt.partial_cmp(&t).unwrap_or(std::cmp::Ordering::Equal));
    let (lo, hi) = match idx {
        Ok(i) => return value_to_vec3f(&samples[i].1),
        Err(i) => (i - 1, i),
    };
    let (tl, vl) = &samples[lo];
    let (th, vh) = &samples[hi];
    let al = value_to_vec3f(vl)?;
    let ah = value_to_vec3f(vh)?;
    let u = ((t - tl) / (th - tl)) as f32;
    Some([
        al[0] + (ah[0] - al[0]) * u,
        al[1] + (ah[1] - al[1]) * u,
        al[2] + (ah[2] - al[2]) * u,
    ])
}

/// Evaluate a scalar double-valued sample list at `t`. Used for
/// `visibility` (as a token-encoded double? no — but hook exists for
/// double attrs generally).
pub fn sample_double_at(samples: &[(f64, Value)], t: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let t_first = samples.first().unwrap().0;
    let t_last = samples.last().unwrap().0;
    if t <= t_first {
        return value_to_double(&samples.first().unwrap().1);
    }
    if t >= t_last {
        return value_to_double(&samples.last().unwrap().1);
    }
    let idx = samples
        .binary_search_by(|(tt, _)| tt.partial_cmp(&t).unwrap_or(std::cmp::Ordering::Equal));
    let (lo, hi) = match idx {
        Ok(i) => return value_to_double(&samples[i].1),
        Err(i) => (i - 1, i),
    };
    let (tl, vl) = &samples[lo];
    let (th, vh) = &samples[hi];
    let dl = value_to_double(vl)?;
    let dh = value_to_double(vh)?;
    let u = (t - tl) / (th - tl);
    Some(dl + (dh - dl) * u)
}

fn value_to_vec3f(v: &Value) -> Option<[f32; 3]> {
    match v {
        Value::Vec3f(a) => Some(*a),
        Value::Vec3d(a) => Some([a[0] as f32, a[1] as f32, a[2] as f32]),
        _ => None,
    }
}

fn value_to_double(v: &Value) -> Option<f64> {
    match v {
        Value::Double(d) => Some(*d),
        Value::Float(f) => Some(*f as f64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v3d(x: f64, y: f64, z: f64) -> Value {
        Value::Vec3d([x, y, z])
    }

    #[test]
    fn interpolates_vec3_between_samples() {
        let samples: Samples = vec![(0.0, v3d(0.0, 0.0, 0.0)), (10.0, v3d(10.0, 20.0, 0.0))];
        let v = sample_vec3_at(&samples, 5.0).unwrap();
        assert!((v[0] - 5.0).abs() < 1e-5, "got {v:?}");
        assert!((v[1] - 10.0).abs() < 1e-5, "got {v:?}");
        assert!((v[2] - 0.0).abs() < 1e-5, "got {v:?}");
    }

    #[test]
    fn holds_at_endpoints() {
        let samples: Samples = vec![(0.0, v3d(1.0, 2.0, 3.0)), (10.0, v3d(4.0, 5.0, 6.0))];
        assert_eq!(sample_vec3_at(&samples, -5.0), Some([1.0, 2.0, 3.0]));
        assert_eq!(sample_vec3_at(&samples, 100.0), Some([4.0, 5.0, 6.0]));
    }

    #[test]
    fn exact_match_returns_exact_value() {
        let samples: Samples = vec![(0.0, v3d(1.0, 2.0, 3.0)), (10.0, v3d(4.0, 5.0, 6.0))];
        assert_eq!(sample_vec3_at(&samples, 10.0), Some([4.0, 5.0, 6.0]));
    }
}
