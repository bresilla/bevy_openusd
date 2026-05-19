//! USD stage-time playback as a Bevy plugin.
//!
//! `usd_bevy` projects authored animation data onto loaded entities
//! at load time (`UsdAsset::animated_prims`, `UsdAsset::skel_animations`,
//! `UsdSkelAnimDriver` components, stage timeline metadata), but it
//! doesn't *drive* anything — that's the host's job. usdview wires
//! these systems into its own binary; before this module, every
//! other usd_bevy consumer had to duplicate that wiring.
//!
//! [`AnimPlugin`] adds the canonical playback loop:
//!
//! 1. [`tick_stage_time`] advances [`UsdStageTime::seconds`] when
//!    `playing`, wraps at the end of the loaded stage's authored
//!    timeline. On first sight of a non-trivial USD timeline (any
//!    `UsdAsset` with `animated_prims`, `skel_animations`, or an
//!    authored `endTimeCode > startTimeCode`) the clock auto-starts.
//! 2. [`evaluate_animated_prims`] re-samples each prim's authored
//!    `xformOp` tracks and overwrites that entity's `Transform`.
//! 3. [`drive_skel_animations`] re-samples every `UsdSkelAnimDriver`
//!    and writes the resulting per-joint transforms.
//!
//! Multi-USD: the clock is a single global resource. Whichever
//! `UsdAsset` lands first contributes `start/end/fps`; subsequent
//! assets are sampled at the same `currentTimeCode` so a scene with
//! a tractor + hummingbird + ... shares one timeline.

use bevy::prelude::*;

use crate::asset::UsdAsset;
use crate::prim_ref::{UsdPrimRef, UsdSkelAnimDriver};

/// Animation playback clock. `seconds` is monotonic real time since
/// the clock initialised; `currentTimeCode` is `start + seconds * fps`.
#[derive(Resource, Debug, Clone, Copy)]
pub struct UsdStageTime {
    pub seconds: f64,
    pub playing: bool,
    pub start_time_code: f64,
    pub end_time_code: f64,
    pub time_codes_per_second: f64,
    /// Latched `true` after first sync from an asset; subsequent
    /// asset loads do NOT reset the clock (so user scrubs / pause
    /// state survives a hot-reload).
    pub initialized: bool,
}

impl Default for UsdStageTime {
    fn default() -> Self {
        Self {
            seconds: 0.0,
            playing: false,
            start_time_code: 0.0,
            end_time_code: 1.0,
            time_codes_per_second: 24.0,
            initialized: false,
        }
    }
}

impl UsdStageTime {
    pub fn current_time_code(&self) -> f64 {
        self.start_time_code + self.seconds * self.time_codes_per_second
    }

    pub fn duration_seconds(&self) -> f64 {
        (self.end_time_code - self.start_time_code).max(0.0) / self.time_codes_per_second
    }
}

/// Wires the stage-time clock and per-frame evaluation systems.
/// Drop into any app that loads USD via [`UsdPlugin`](crate::UsdPlugin)
/// to get authored `xformOp` and `UsdSkel` animations playing.
#[derive(Default)]
pub struct AnimPlugin;

impl Plugin for AnimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UsdStageTime>().add_systems(
            Update,
            (
                tick_stage_time,
                evaluate_animated_prims.after(tick_stage_time),
                drive_skel_animations.after(tick_stage_time),
            ),
        );
    }
}

/// Initialise the clock from the first non-trivial `UsdAsset` we
/// see, then advance `seconds` while `playing`. Loops at the end of
/// the authored timeline (no end-hold; held-interpolation samples
/// rendered for one frame at the wrap is acceptable).
pub fn tick_stage_time(
    time: Res<Time>,
    mut clock: ResMut<UsdStageTime>,
    usd_assets: Res<Assets<UsdAsset>>,
) {
    if !clock.initialized {
        // Pick the asset whose authored timeline is the LONGEST and
        // carries actual animation (`animated_prims` or
        // `skel_animations`). A stage may set `endTimeCode` to a
        // tiny default with nothing to animate (e.g. franka has
        // `start=-1, end=0`, duration ≈ 0.02s); using that as the
        // global clock loops every other frame and starves any real
        // animation in a sibling asset (hummingbird wings, etc.).
        let mut best: Option<(f64, &UsdAsset)> = None;
        for (_id, asset) in usd_assets.iter() {
            let dur = (asset.end_time_code - asset.start_time_code).max(0.0)
                / asset.time_codes_per_second.max(1e-6);
            let has_real_anim =
                !asset.animated_prims.is_empty() || !asset.skel_animations.is_empty();
            // Prefer assets that have real animation content; only
            // fall back to "stage with authored timeline" if nothing
            // else is loaded yet. Either way we maximise duration.
            let score = if has_real_anim {
                dur + 1_000_000.0
            } else {
                dur
            };
            if best.is_none_or(|(b, _)| score > b) {
                best = Some((score, asset));
            }
        }
        if let Some((_, asset)) = best {
            // Demand at least a meaningful timeline before locking
            // the clock in. Otherwise wait for a later asset to land.
            let dur = (asset.end_time_code - asset.start_time_code).max(0.0)
                / asset.time_codes_per_second.max(1e-6);
            let has_real_anim =
                !asset.animated_prims.is_empty() || !asset.skel_animations.is_empty();
            if has_real_anim || dur > 0.1 {
                clock.start_time_code = asset.start_time_code;
                clock.end_time_code = asset.end_time_code;
                clock.time_codes_per_second = asset.time_codes_per_second;
                clock.seconds = 0.0;
                clock.playing = true;
                clock.initialized = true;
                info!(
                    "stage time clock: start={:.2} end={:.2} fps={:.2} (duration {:.2}s) — {} animated prim(s), {} skel anim(s)",
                    clock.start_time_code,
                    clock.end_time_code,
                    clock.time_codes_per_second,
                    clock.duration_seconds(),
                    asset.animated_prims.len(),
                    asset.skel_animations.len()
                );
            }
        }
    }

    if clock.playing {
        clock.seconds += time.delta_secs_f64();
        let dur = clock.duration_seconds();
        if dur > 0.0 && clock.seconds >= dur {
            clock.seconds = clock.seconds.rem_euclid(dur);
        }
    }
}

/// Re-evaluate authored single-axis `xformOp:rotate*` tracks on
/// every prim across every loaded `UsdAsset`, writing the sampled
/// orientation into that entity's `Transform`. Only prims whose
/// path appears in some asset's `animated_prims` map are touched —
/// static geometry pays one HashMap lookup per frame.
pub fn evaluate_animated_prims(
    clock: Res<UsdStageTime>,
    usd_assets: Res<Assets<UsdAsset>>,
    mut prims: Query<(&UsdPrimRef, &mut Transform)>,
) {
    if usd_assets.is_empty() {
        return;
    }
    let tc = clock.current_time_code();
    use usd_schema::anim::eval_scalar_track;

    for (prim_ref, mut tr) in prims.iter_mut() {
        // Multi-USD: walk every loaded asset to find this prim's
        // animated record. Same prim path in two assets is unusual
        // but valid (referenced layers); first match wins.
        let mut record = None;
        for (_id, asset) in usd_assets.iter() {
            if let Some(r) = asset.animated_prims.get(&prim_ref.path) {
                record = Some(r);
                break;
            }
        }
        let Some(record) = record else {
            continue;
        };
        if let Some(track) = &record.rotate_y
            && let Some(deg) = eval_scalar_track(track, tc)
        {
            tr.rotation = bevy::math::Quat::from_rotation_y(deg.to_radians());
        }
        if let Some(track) = &record.rotate_x
            && let Some(deg) = eval_scalar_track(track, tc)
        {
            tr.rotation = bevy::math::Quat::from_rotation_x(deg.to_radians());
        }
        if let Some(track) = &record.rotate_z
            && let Some(deg) = eval_scalar_track(track, tc)
        {
            tr.rotation = bevy::math::Quat::from_rotation_z(deg.to_radians());
        }
    }
}

/// Sample every `UsdSkelAnimation` driver at the current time code
/// and write the per-joint TRS values into the resolved joint
/// entities. No-op when no skel-animated assets are loaded.
pub fn drive_skel_animations(
    clock: Res<UsdStageTime>,
    drivers: Query<&UsdSkelAnimDriver>,
    mut joints: Query<&mut Transform>,
) {
    let tc = clock.current_time_code();
    for driver in drivers.iter() {
        let evaluated = crate::skel_anim::evaluate(driver, tc);
        for (channel_ix, joint_entity) in driver.joint_entities.iter().enumerate() {
            let Some(je) = joint_entity else {
                continue;
            };
            let Ok(mut tr) = joints.get_mut(*je) else {
                continue;
            };
            evaluated[channel_ix].apply(&mut tr);
        }
    }
}
