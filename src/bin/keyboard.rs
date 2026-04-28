//! Keyboard shortcuts: panel toggles + overlay toggles.
//!
//! Run-if `not(egui_wants_any_pointer_input)` so typing in a future search
//! field doesn't also toggle panels.

use bevy::prelude::*;
use bevy_egui::input::egui_wants_any_keyboard_input;
use bevy_frost::RibbonOpen;

use crate::overlays::DisplayToggles;
use crate::state::ReloadRequest;
use crate::ui::{ViewerCommandPalette, RIBBON_LEFT, RIB_INFO, RIB_KEYS, RIB_OVERLAYS, RIB_TREE};

pub struct ViewerKeyboardPlugin;

impl Plugin for ViewerKeyboardPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                handle_keys.run_if(not(egui_wants_any_keyboard_input)),
                handle_palette_shortcut,
            ),
        );
    }
}

/// Ctrl+K / Ctrl+P opens or closes the command palette. Runs
/// unguarded by egui's keyboard grab so the shortcut works while
/// the palette itself has focus (closing it via the same chord).
fn handle_palette_shortcut(
    keys: Res<ButtonInput<KeyCode>>,
    mut palette: ResMut<ViewerCommandPalette>,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    if !ctrl {
        return;
    }
    if keys.just_pressed(KeyCode::KeyK) || keys.just_pressed(KeyCode::KeyP) {
        palette.0.open = !palette.0.open;
        if palette.0.open {
            palette.0.query.clear();
            palette.0.selected = 0;
        }
    }
}

fn handle_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut ribbon: ResMut<RibbonOpen>,
    mut toggles: ResMut<DisplayToggles>,
    mut reload: ResMut<ReloadRequest>,
) {
    if keys.just_pressed(KeyCode::KeyT) {
        ribbon.toggle(RIBBON_LEFT, RIB_TREE);
    }
    if keys.just_pressed(KeyCode::KeyI) {
        ribbon.toggle(RIBBON_LEFT, RIB_INFO);
    }
    if keys.just_pressed(KeyCode::KeyO) {
        ribbon.toggle(RIBBON_LEFT, RIB_OVERLAYS);
    }
    // Both `/` and `?` sit on the same physical key.
    if keys.just_pressed(KeyCode::Slash) {
        ribbon.toggle(RIBBON_LEFT, RIB_KEYS);
    }

    if keys.just_pressed(KeyCode::KeyG) {
        toggles.show_world_grid = !toggles.show_world_grid;
    }
    if keys.just_pressed(KeyCode::KeyX) {
        toggles.show_world_axes = !toggles.show_world_axes;
    }
    if keys.just_pressed(KeyCode::KeyP) {
        toggles.show_prim_markers = !toggles.show_prim_markers;
    }
    if keys.just_pressed(KeyCode::KeyB) {
        toggles.show_skeleton = !toggles.show_skeleton;
    }
    if keys.just_pressed(KeyCode::KeyR) {
        reload.requested = true;
    }
}
