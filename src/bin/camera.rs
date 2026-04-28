//! Orbit camera — ported verbatim from
//! `../bevy_urdf/src/camera.rs`. Astrocraft-style rig:
//!
//! - Scroll            → zoom (logarithmic, smoothed)
//! - Middle-click drag → pan (focus slides in world XZ plane)
//! - Left + Right drag → orbit (yaw + elevation, clamped 5°–89°)
//!
//! Run conditions yield to egui when a panel wants the pointer, so orbiting
//! doesn't fight with panel scroll / sliders.

use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::input::egui_wants_any_pointer_input;

use crate::state::CameraMount;

pub struct ArcballCameraPlugin;

impl Plugin for ArcballCameraPlugin {
    fn build(&self, app: &mut App) {
        // Arcball yields to egui (so scrolling a panel doesn't zoom) AND
        // to the Cameras tab (so a mounted USD camera isn't fought by
        // orbit/pan input). Tiny run-condition saves a lot of confusion.
        app.add_systems(
            Update,
            (drive_arcball, drive_arcball_zoom)
                .run_if(not(egui_wants_any_pointer_input))
                .run_if(arcball_is_active),
        );
    }
}

fn arcball_is_active(mount: Res<CameraMount>) -> bool {
    matches!(*mount, CameraMount::Arcball)
}

#[derive(Component, Clone)]
pub struct ArcballCamera {
    pub focus: Vec3,
    pub yaw: f32,
    pub elevation: f32,
    pub distance: f32,
    pub min_distance: f32,
    pub max_distance: f32,
    pub pan_sensitivity: f32,
    pub orbit_speed: f32,
    pub zoom_step: f64,
    pub zoom_smoothing: f64,
}

impl Default for ArcballCamera {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            yaw: 0.0,
            elevation: 25f32.to_radians(),
            distance: 4.0,
            min_distance: 0.2,
            max_distance: 60.0,
            pan_sensitivity: 0.0012,
            orbit_speed: 0.005,
            zoom_step: 0.05,
            zoom_smoothing: 6.0,
        }
    }
}

fn drive_arcball(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    mut pan_anchor: Local<Option<Vec2>>,
    mut orbit_anchor: Local<Option<Vec2>>,
    mut cameras: Query<(&mut Transform, &mut ArcballCamera)>,
) {
    let modifier_held = keys.any_pressed([
        KeyCode::ShiftLeft,
        KeyCode::ShiftRight,
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
    ]);

    let middle = !modifier_held && mouse_buttons.pressed(MouseButton::Middle);
    let left = !modifier_held && mouse_buttons.pressed(MouseButton::Left);
    let right = !modifier_held && mouse_buttons.pressed(MouseButton::Right);
    let both_lr = left && right;

    if !middle {
        *pan_anchor = None;
    }
    if !both_lr {
        *orbit_anchor = None;
    }

    let cursor = primary_window
        .single()
        .ok()
        .and_then(|w| w.cursor_position());

    let mut pan_delta = Vec2::ZERO;
    if middle {
        if let Some(pos) = cursor {
            if let Some(anchor) = *pan_anchor {
                pan_delta = pos - anchor;
            }
            *pan_anchor = Some(pos);
        }
    }

    let mut orbit_delta = Vec2::ZERO;
    if both_lr {
        if let Some(pos) = cursor {
            if let Some(anchor) = *orbit_anchor {
                orbit_delta = pos - anchor;
            }
            *orbit_anchor = Some(pos);
        }
    }

    for (mut tr, mut cam) in cameras.iter_mut() {
        if pan_delta != Vec2::ZERO {
            let pan_speed = cam.distance * cam.pan_sensitivity;
            let forward = Vec3::new(cam.yaw.sin(), 0.0, cam.yaw.cos());
            let right_v = Vec3::new(forward.z, 0.0, -forward.x);
            cam.focus += (-right_v * pan_delta.x - forward * pan_delta.y) * pan_speed;
        }
        if orbit_delta != Vec2::ZERO {
            cam.yaw -= orbit_delta.x * cam.orbit_speed;
            cam.elevation += orbit_delta.y * cam.orbit_speed;
            cam.elevation = cam.elevation.clamp(5f32.to_radians(), 89f32.to_radians());
        }
        apply_rig(&cam, &mut tr);
    }
}

fn drive_arcball_zoom(
    time: Res<Time>,
    scroll: Res<AccumulatedMouseScroll>,
    mut zoom_target: Local<Option<f64>>,
    mut cameras: Query<(&mut Transform, &mut ArcballCamera)>,
) {
    let scroll_delta: f64 = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y as f64,
        MouseScrollUnit::Pixel => scroll.delta.y as f64 / 32.0,
    };

    for (mut tr, mut cam) in cameras.iter_mut() {
        let target = zoom_target.get_or_insert(cam.distance as f64);
        let min = cam.min_distance as f64;
        let max = cam.max_distance as f64;

        if scroll_delta != 0.0 {
            let log_target = target.max(0.01).log10();
            let new_log = log_target - scroll_delta * cam.zoom_step;
            *target = 10f64.powf(new_log).clamp(min, max);
        }

        let dt = time.delta_secs_f64();
        let log_current = (cam.distance as f64).max(0.01).ln();
        let log_target = target.max(0.01).ln();
        let log_diff = log_target - log_current;
        if log_diff.abs() > 1e-4 {
            let new_log = log_current + log_diff * (cam.zoom_smoothing * dt).min(0.9);
            cam.distance = new_log.exp() as f32;
            apply_rig(&cam, &mut tr);
        } else if log_diff.abs() > 1e-5 {
            cam.distance = *target as f32;
            apply_rig(&cam, &mut tr);
        }
    }
}

fn apply_rig(cam: &ArcballCamera, tr: &mut Transform) {
    let horizontal = cam.distance * cam.elevation.cos();
    let vertical = cam.distance * cam.elevation.sin();
    let offset = Vec3::new(
        horizontal * cam.yaw.sin(),
        vertical,
        horizontal * cam.yaw.cos(),
    );
    let cam_world = cam.focus + offset;
    *tr = Transform::from_translation(cam_world).looking_at(cam.focus, Vec3::Y);
}
