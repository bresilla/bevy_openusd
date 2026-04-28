//! Viewer UI — bevy_frost ribbons + floating panels + widgets.
//!
//! Left rail is one `TwoSided` panel ribbon. Primary tools live in the
//! `Start` cluster (top-anchored); utility/help tools live in the `End`
//! cluster (bottom-anchored). Panel visibility is driven by the
//! `RibbonOpen` resource that frost ships with — clicking a rail button
//! toggles exclusively.
//!
//! PaneBuilder constraint: every pane body may ONLY call
//! `pane.section(id, title, default_open, body)`. Any free-standing
//! widget (sub_caption, readout_row, ScrollArea, …) must live inside
//! that body — which receives a regular `&mut egui::Ui`.

use bevy::asset::Assets;
use bevy::ecs::hierarchy::Children;
use bevy::mesh::Mesh3d;
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use bevy_frost::prelude::*;
use bevy_frost::style;
use bevy_frost::widgets::section as nested_section;
use bevy_openusd::{UsdAsset, UsdDisplayName, UsdKind, UsdPrimRef, UsdProcedural, UsdSpatialAudio};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::camera::ArcballCamera;
use crate::overlays::DisplayToggles;
use crate::state::{
    CameraBookmark, CameraBookmarks, CameraMount, FlyTo, LoadRequest, LoaderTuning, ReloadRequest,
    SelectedPrim, StageInfo, UsdStageTime,
};

// ─── Ribbon declaration ─────────────────────────────────────────────

pub const RIBBON_LEFT: &str = "viewer_left";

pub const RIB_SELECTION: &str = "viewer_selection";
pub const RIB_TREE: &str = "viewer_tree";
pub const RIB_INFO: &str = "viewer_info";
pub const RIB_VARIANTS: &str = "viewer_variants";
pub const RIB_CAMERAS: &str = "viewer_cameras";
pub const RIB_OVERLAYS: &str = "viewer_overlays";
pub const RIB_TIMELINE: &str = "viewer_timeline";
pub const RIB_KEYS: &str = "viewer_keys";
pub const RIB_LOG: &str = "viewer_log";

const RIBBONS: &[RibbonDef] = &[RibbonDef {
    id: RIBBON_LEFT,
    edge: RibbonEdge::Left,
    role: RibbonRole::Panel,
    mode: RibbonMode::TwoSided,
    draggable: false,
    accepts: &[],
}];

const RIBBON_ITEMS: &[RibbonItem] = &[
    RibbonItem {
        id: RIB_SELECTION,
        ribbon: RIBBON_LEFT,
        cluster: RibbonCluster::Start,
        slot: 0,
        glyph: "F",
        tooltip: "File / selection",
        child_ribbon: None,
    },
    RibbonItem {
        id: RIB_TREE,
        ribbon: RIBBON_LEFT,
        cluster: RibbonCluster::Start,
        slot: 1,
        glyph: "T",
        tooltip: "Prim tree (T)",
        child_ribbon: None,
    },
    RibbonItem {
        id: RIB_INFO,
        ribbon: RIBBON_LEFT,
        cluster: RibbonCluster::Start,
        slot: 2,
        glyph: "i",
        tooltip: "Stage info (I)",
        child_ribbon: None,
    },
    RibbonItem {
        id: RIB_VARIANTS,
        ribbon: RIBBON_LEFT,
        cluster: RibbonCluster::Start,
        slot: 3,
        glyph: "V",
        tooltip: "Variants",
        child_ribbon: None,
    },
    RibbonItem {
        id: RIB_CAMERAS,
        ribbon: RIBBON_LEFT,
        cluster: RibbonCluster::Start,
        slot: 4,
        glyph: "C",
        tooltip: "Cameras",
        child_ribbon: None,
    },
    RibbonItem {
        id: RIB_OVERLAYS,
        ribbon: RIBBON_LEFT,
        cluster: RibbonCluster::End,
        slot: 0,
        glyph: "O",
        tooltip: "Overlays (O)",
        child_ribbon: None,
    },
    RibbonItem {
        id: RIB_TIMELINE,
        ribbon: RIBBON_LEFT,
        cluster: RibbonCluster::End,
        slot: 1,
        glyph: "⏱",
        tooltip: "Timeline",
        child_ribbon: None,
    },
    RibbonItem {
        id: RIB_KEYS,
        ribbon: RIBBON_LEFT,
        cluster: RibbonCluster::End,
        slot: 2,
        glyph: "?",
        tooltip: "Controls (?)",
        child_ribbon: None,
    },
    RibbonItem {
        id: RIB_LOG,
        ribbon: RIBBON_LEFT,
        cluster: RibbonCluster::End,
        slot: 3,
        glyph: "📜",
        tooltip: "Log",
        child_ribbon: None,
    },
];

/// Prim-tree expansion state, keyed by `UsdPrimRef.path`. Entries
/// default to expanded the first time a row is rendered.
#[derive(Resource, Default)]
pub struct TreeExpanded(pub HashMap<String, bool>);

/// Free-text filter for the prim-tree panel. When non-empty, the
/// panel switches to a flat-list mode showing every prim whose path
/// contains the substring (case-insensitive).
#[derive(Resource, Default)]
pub struct TreeFilter(pub String);

/// Wrapper around frost's `CommandPaletteState` so Bevy can track it
/// as a Resource without needing to derive on an upstream type.
#[derive(Resource, Default)]
pub struct ViewerCommandPalette(pub CommandPaletteState);

/// The palette's static action list. Adding a new id here only
/// requires a matching arm in `dispatch_palette` below.
const PALETTE_ITEMS: &[PaletteItem] = &[
    PaletteItem { id: "open_selection", label: "Open: Selection panel", hint: Some("F") },
    PaletteItem { id: "open_tree", label: "Open: Prim tree", hint: Some("T") },
    PaletteItem { id: "open_info", label: "Open: Stage info", hint: Some("I") },
    PaletteItem { id: "open_variants", label: "Open: Variants", hint: None },
    PaletteItem { id: "open_cameras", label: "Open: Cameras", hint: None },
    PaletteItem { id: "open_overlays", label: "Open: Overlays", hint: Some("O") },
    PaletteItem { id: "open_timeline", label: "Open: Timeline", hint: None },
    PaletteItem { id: "open_keys", label: "Open: Controls", hint: Some("?") },
    PaletteItem { id: "open_log", label: "Open: Log", hint: None },
    PaletteItem { id: "toggle_grid", label: "Toggle: Ground grid", hint: Some("G") },
    PaletteItem { id: "toggle_axes", label: "Toggle: World axes", hint: Some("X") },
    PaletteItem { id: "toggle_markers", label: "Toggle: Prim markers", hint: Some("P") },
    PaletteItem { id: "toggle_wireframe", label: "Toggle: Wireframe", hint: None },
    PaletteItem { id: "reload_stage", label: "Stage: Reload", hint: Some("R") },
    PaletteItem { id: "browse_usd", label: "Stage: Browse for USD…", hint: None },
];

// ─── Plugin ─────────────────────────────────────────────────────────

pub struct ViewerUiPlugin;

impl Plugin for ViewerUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bevy_frost::FrostPlugin>() {
            app.add_plugins(bevy_frost::FrostPlugin);
        }
        app.init_resource::<TreeExpanded>()
            .init_resource::<TreeFilter>()
            .init_resource::<ViewerCommandPalette>()
            .add_systems(
                EguiPrimaryContextPass,
                (
                    draw_ribbons,
                    draw_selection_panel,
                    draw_tree_panel,
                    draw_info_panel,
                    draw_variants_panel,
                    draw_cameras_panel,
                    draw_overlays_panel,
                    draw_timeline_panel,
                    draw_keys_panel,
                    draw_log_panel,
                    draw_palette_panel,
                    draw_status_bar,
                )
                    .chain(),
            );
    }
}

const PANEL_W: f32 = 340.0;
const PANEL_H: f32 = 560.0;

// ─── Ribbon rail ────────────────────────────────────────────────────

fn draw_ribbons(
    mut contexts: EguiContexts,
    accent: Res<AccentColor>,
    mut open: ResMut<RibbonOpen>,
    mut placement: ResMut<RibbonPlacement>,
    mut drag: ResMut<RibbonDrag>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let _clicks = draw_assembly(
        ctx,
        accent.0,
        RIBBONS,
        RIBBON_ITEMS,
        &mut open,
        &mut placement,
        &mut drag,
        |_| false,
    );
}

fn is_panel_open(open: &RibbonOpen, item: &'static str) -> bool {
    open.is_open(RIBBON_LEFT, item)
}

// ─── Selection panel ────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_selection_panel(
    mut contexts: EguiContexts,
    open: Res<RibbonOpen>,
    placement: Res<RibbonPlacement>,
    accent: Res<AccentColor>,
    info: Res<StageInfo>,
    requested: Res<crate::RequestedAsset>,
    mut load_req: ResMut<LoadRequest>,
    mut selected: ResMut<SelectedPrim>,
    prims: Query<(Entity, &Name, &UsdPrimRef)>,
    mesh_q: Query<(), With<Mesh3d>>,
    kind_q: Query<&UsdKind>,
    audio_q: Query<&UsdSpatialAudio>,
    proc_q: Query<&UsdProcedural>,
    vis_q: Query<&Visibility>,
    children: Query<&Children>,
) {
    if !is_panel_open(&open, RIB_SELECTION) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let accent_col = accent.0;
    let mut keep = true;
    floating_window_for_item(
        ctx,
        RIBBONS,
        RIBBON_ITEMS,
        &placement,
        RIB_SELECTION,
        "Selection",
        egui::vec2(PANEL_W, PANEL_H),
        &mut keep,
        accent_col,
        |pane| {
            pane.section("sel_stage", "Loaded stage", true, |ui| {
                readout_row(ui, "file", info.path.as_str());
                if wide_button(ui, "📁  Browse USD…", accent_col).clicked()
                    && let Some(picked) = rfd::FileDialog::new()
                        .add_filter("USD stages", &["usda", "usdc", "usd", "usdz"])
                        .pick_file()
                {
                    load_req.path = Some(PathBuf::from(picked));
                }
                if wide_button(ui, "🗂  Reveal in filesystem", accent_col).clicked() {
                    let full = requested.root.join(&info.path);
                    let target = full.parent().unwrap_or(&requested.root).to_path_buf();
                    let _ = std::process::Command::new("xdg-open").arg(&target).spawn();
                }
            });
            pane.section("sel_prim", "Selected prim", true, |ui| match selected.0 {
                Some(entity) => {
                    if let Ok((_, n, pr)) = prims.get(entity) {
                        readout_row(ui, "name", n.as_str());
                        readout_row(ui, "path", pr.path.as_str());

                        // Feature chips — derived purely from ECS
                        // component presence so the row stays in sync
                        // with the live stage without a dedicated
                        // cache.
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            if mesh_q.get(entity).is_ok() {
                                chip(ui, "mesh", accent_col);
                            }
                            if let Ok(k) = kind_q.get(entity) {
                                chip(ui, &format!("kind:{}", k.kind), accent_col);
                            }
                            if children.get(entity).map(|c| !c.is_empty()).unwrap_or(false) {
                                chip(ui, "parent", accent_col);
                            }
                            if audio_q.get(entity).is_ok() {
                                chip(ui, "audio", accent_col);
                            }
                            if proc_q.get(entity).is_ok() {
                                chip(ui, "procedural", accent_col);
                            }
                            if matches!(vis_q.get(entity), Ok(Visibility::Hidden)) {
                                chip_colored(ui, "hidden", style::WARNING, accent_col);
                            }
                        });

                        if wide_button(ui, "Clear selection", accent_col).clicked() {
                            selected.0 = None;
                        }
                    } else {
                        sub_caption(ui, "(selection stale)");
                        selected.0 = None;
                    }
                }
                None => sub_caption(ui, "Click a prim in the Tree panel"),
            });
        },
    );
}

// ─── Prim-tree panel ────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_tree_panel(
    mut contexts: EguiContexts,
    open: Res<RibbonOpen>,
    placement: Res<RibbonPlacement>,
    accent: Res<AccentColor>,
    mut selected: ResMut<SelectedPrim>,
    mut fly: ResMut<FlyTo>,
    mut expanded: ResMut<TreeExpanded>,
    mut filter: ResMut<TreeFilter>,
    materials: Res<Assets<StandardMaterial>>,
    cameras: Query<&ArcballCamera>,
    gt_query: Query<&GlobalTransform>,
    extent_q: Query<&bevy_openusd::UsdLocalExtent>,
    // Combined with `Option<&UsdDisplayName>` so the system stays
    // under Bevy's 16-SystemParam limit. The recursive row helper
    // pulls the display name via `prims.get(entity)` instead of
    // a separate query.
    prims: Query<(Entity, &Name, &UsdPrimRef, Option<&UsdDisplayName>)>,
    mat_q: Query<&MeshMaterial3d<StandardMaterial>>,
    mut visibility_q: Query<(Entity, &mut Visibility)>,
    children: Query<&Children>,
) {
    if !is_panel_open(&open, RIB_TREE) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let accent_col = accent.0;
    let mut keep = true;
    floating_window_for_item(
        ctx,
        RIBBONS,
        RIBBON_ITEMS,
        &placement,
        RIB_TREE,
        "Prim tree",
        egui::vec2(PANEL_W, 720.0),
        &mut keep,
        accent_col,
        |pane| {
            pane.section("tree_hierarchy", "Hierarchy", true, |ui| {
                sub_caption(ui, &format!("{} prims", prims.iter().count()));
                ui.add_space(style::space::TIGHT);
                search_field(ui, &mut filter.0, "Search prims…", accent_col);
                ui.add_space(style::space::BLOCK);

                // Snapshot the current Visibility state so the tree
                // rows can drive eye-icon toggles via plain &mut bool
                // — we commit changes back to the ECS once the row
                // rendering is finished.
                let mut vis_cache: HashMap<Entity, bool> = HashMap::new();
                for (e, v) in visibility_q.iter() {
                    vis_cache.insert(e, !matches!(*v, Visibility::Hidden));
                }
                // `visibility_q.get_mut(e)` below returns
                // `Result<(Entity, Mut<Visibility>)>`; we only need
                // the Mut half, hence the destructuring pattern.
                let vis_before = vis_cache.clone();

                let filter_lc = filter.0.to_lowercase();
                let flat = !filter_lc.is_empty();

                let mut outcome = RowOutcome::default();
                // Hardcoded generous viewport — frost's `section`
                // allocates the body Ui with initial height 0, so
                // `available_height` here would clip the scroll list
                // to almost nothing. 600 px gives ~30 visible rows
                // at the default `TREE_ROW_H = 20`; the panel itself
                // opens 720 px tall so this fits without overflow.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .min_scrolled_height(600.0)
                    .max_height(600.0)
                    .show(ui, |ui| {
                    if flat {
                        let mut matches: Vec<(Entity, &Name, &UsdPrimRef, Option<&UsdDisplayName>)> =
                            prims
                                .iter()
                                .filter(|(_, _, pref, _)| {
                                    pref.path.to_lowercase().contains(&filter_lc)
                                })
                                .collect();
                        matches.sort_by(|a, b| a.2.path.cmp(&b.2.path));
                        if matches.is_empty() {
                            sub_caption(ui, "(no matches)");
                        }
                        for (entity, name, pref, dn) in &matches {
                            let sub = draw_tree_row(
                                ui,
                                *entity,
                                name,
                                pref,
                                *dn,
                                &prims,
                                &mat_q,
                                &materials,
                                &mut vis_cache,
                                &children,
                                &selected,
                                &mut expanded,
                                accent_col,
                                0,
                                true,
                            );
                            outcome.merge(sub);
                        }
                    } else {
                        let mut roots: Vec<(Entity, &Name, &UsdPrimRef, Option<&UsdDisplayName>)> =
                            prims
                                .iter()
                                .filter(|(_, _, pref, _)| {
                                    let p = pref.path.as_str();
                                    p.starts_with('/') && p.len() > 1 && !p[1..].contains('/')
                                })
                                .collect();
                        roots.sort_by(|a, b| a.2.path.cmp(&b.2.path));

                        if roots.is_empty() {
                            sub_caption(ui, "(no prims yet — stage loading)");
                        } else {
                            for (entity, name, pref, dn) in &roots {
                                let sub = draw_tree_row(
                                    ui,
                                    *entity,
                                    name,
                                    pref,
                                    *dn,
                                    &prims,
                                    &mat_q,
                                    &materials,
                                    &mut vis_cache,
                                    &children,
                                    &selected,
                                    &mut expanded,
                                    accent_col,
                                    0,
                                    false,
                                );
                                outcome.merge(sub);
                            }
                        }
                    }
                });

                // Commit eye-icon toggles back to the ECS.
                for (entity, visible) in &vis_cache {
                    if vis_before.get(entity) != Some(visible) {
                        if let Ok((_, mut v)) = visibility_q.get_mut(*entity) {
                            *v = if *visible {
                                Visibility::Inherited
                            } else {
                                Visibility::Hidden
                            };
                        }
                    }
                }

                if let Some(action) = outcome.ctx_action {
                    match action {
                        CtxAction::FlyTo(entity) => {
                            selected.0 = Some(entity);
                            if let (Ok(target_gt), Ok(cam)) =
                                (gt_query.get(entity), cameras.single())
                            {
                                let target = target_gt.translation();
                                let target_dist =
                                    (cam.distance * 0.25).clamp(0.2, 40.0);
                                fly.start_focus = cam.focus;
                                fly.start_distance = cam.distance;
                                fly.target_focus = target;
                                fly.target_distance = target_dist;
                                fly.duration = 0.4;
                                fly.remaining = 0.4;
                            }
                        }
                        CtxAction::Fit(entity) => {
                            selected.0 = Some(entity);
                            if let Ok(cam) = cameras.single() {
                                let (target, target_dist) = fit_params_for_entity(
                                    entity, &gt_query, &extent_q, &children, cam.distance,
                                );
                                fly.start_focus = cam.focus;
                                fly.start_distance = cam.distance;
                                fly.target_focus = target;
                                fly.target_distance = target_dist;
                                fly.duration = 0.4;
                                fly.remaining = 0.4;
                            }
                        }
                        CtxAction::ExpandDesc(entity) => {
                            set_subtree_expanded(
                                entity, &prims, &children, &mut expanded, true,
                            );
                        }
                        CtxAction::CollapseDesc(entity) => {
                            set_subtree_expanded(
                                entity, &prims, &children, &mut expanded, false,
                            );
                        }
                    }
                }

                if let Some(entity) = outcome.double_clicked {
                    selected.0 = Some(entity);
                    if let Ok(cam) = cameras.single() {
                        let (target, target_dist) = fit_params_for_entity(
                            entity, &gt_query, &extent_q, &children, cam.distance,
                        );
                        fly.start_focus = cam.focus;
                        fly.start_distance = cam.distance;
                        fly.target_focus = target;
                        fly.target_distance = target_dist;
                        fly.duration = 0.4;
                        fly.remaining = 0.4;
                    }
                } else if let Some(entity) = outcome.clicked {
                    selected.0 = Some(entity);
                    if let (Ok(target_gt), Ok(cam)) = (gt_query.get(entity), cameras.single()) {
                        let target = target_gt.translation();
                        let target_dist = (cam.distance * 0.25).clamp(0.2, 40.0);
                        fly.start_focus = cam.focus;
                        fly.start_distance = cam.distance;
                        fly.target_focus = target;
                        fly.target_distance = target_dist;
                        fly.duration = 0.4;
                        fly.remaining = 0.4;
                    }
                }
            });
        },
    );
}

#[derive(Default, Clone, Copy)]
struct RowOutcome {
    clicked: Option<Entity>,
    double_clicked: Option<Entity>,
    ctx_action: Option<CtxAction>,
}

#[derive(Clone, Copy, Debug)]
enum CtxAction {
    FlyTo(Entity),
    Fit(Entity),
    ExpandDesc(Entity),
    CollapseDesc(Entity),
}

impl RowOutcome {
    fn merge(&mut self, other: RowOutcome) {
        if other.double_clicked.is_some() {
            self.double_clicked = other.double_clicked;
        }
        if other.clicked.is_some() {
            self.clicked = other.clicked;
        }
        if other.ctx_action.is_some() {
            self.ctx_action = other.ctx_action;
        }
    }
}

/// Walk the subtree rooted at `root` and set each descendant's
/// `TreeExpanded` entry to `open`. Used by the row context-menu
/// "Expand / Collapse descendants" actions.
fn set_subtree_expanded(
    root: Entity,
    prims: &Query<(Entity, &Name, &UsdPrimRef, Option<&UsdDisplayName>)>,
    children: &Query<&Children>,
    expanded: &mut TreeExpanded,
    open: bool,
) {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if let Ok((_, _, pref, _)) = prims.get(e) {
            expanded.0.insert(pref.path.clone(), open);
        }
        if let Ok(cs) = children.get(e) {
            for c in cs.iter() {
                stack.push(c);
            }
        }
    }
}

/// Lookup the first-bound material's `base_color` for `entity` (or
/// one of its direct mesh-carrying children) and convert linear sRGB
/// into an egui colour suitable for a tree-row swatch.
fn swatch_color_for(
    entity: Entity,
    mat_q: &Query<&MeshMaterial3d<StandardMaterial>>,
    children: &Query<&Children>,
    materials: &Assets<StandardMaterial>,
) -> Option<egui::Color32> {
    let pick = |e: Entity| -> Option<egui::Color32> {
        let mm = mat_q.get(e).ok()?;
        let mat = materials.get(&mm.0)?;
        let c = mat.base_color.to_linear();
        Some(style::srgb_to_egui([c.red, c.green, c.blue]))
    };
    if let Some(c) = pick(entity) {
        return Some(c);
    }
    if let Ok(cs) = children.get(entity) {
        for c in cs.iter() {
            if let Some(col) = pick(c) {
                return Some(col);
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn draw_tree_row(
    ui: &mut egui::Ui,
    entity: Entity,
    name: &Name,
    prim_ref: &UsdPrimRef,
    display_name: Option<&UsdDisplayName>,
    prims: &Query<(Entity, &Name, &UsdPrimRef, Option<&UsdDisplayName>)>,
    mat_q: &Query<&MeshMaterial3d<StandardMaterial>>,
    materials: &Assets<StandardMaterial>,
    vis_cache: &mut HashMap<Entity, bool>,
    children: &Query<&Children>,
    selected: &SelectedPrim,
    expanded: &mut TreeExpanded,
    accent: egui::Color32,
    depth: u32,
    // Force a leaf-style row (no chevron, no descendants). Used by
    // the flat filter mode where we render ancestorless hits.
    leaf_override: bool,
) -> RowOutcome {
    let child_ids: Vec<Entity> = children
        .get(entity)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    let mut prim_children: Vec<(Entity, &Name, &UsdPrimRef, Option<&UsdDisplayName>)> = child_ids
        .iter()
        .filter_map(|c| prims.get(*c).ok())
        .collect();
    prim_children.sort_by(|a, b| a.2.path.cmp(&b.2.path));
    let has_children = !leaf_override && !prim_children.is_empty();

    let is_selected = selected.0 == Some(entity);
    let path_key = prim_ref.path.clone();
    // Tree-row egui id: entity's bits, NOT the prim path. Production
    // assets (Davinci, PointInstancer expansion, internal-reference
    // dedup) routinely produce multiple entities sharing one prim
    // path; using the path as id_salt collides those rows in egui's
    // internal id arena and blasts the console with "ID is not
    // unique" warnings. Entity IDs are guaranteed unique within the
    // ECS world — perfect.
    let row_id_salt = entity.to_bits();
    let mut outcome = RowOutcome::default();

    // Eye + swatch slots.
    let mut visible_flag = *vis_cache.get(&entity).unwrap_or(&true);
    let swatch = swatch_color_for(entity, mat_q, children, materials);
    let mut color_sentinel = false;

    // Label preference: authored `ui:displayName` (UsdUI) > prim leaf
    // name. Most stages won't author a display name and fall straight
    // through to the leaf.
    let label_owned: String = display_name
        .map(|d| d.0.clone())
        .unwrap_or_else(|| name.as_str().to_string());

    let resp = {
        let mut slot_buf: Vec<TreeIconSlot<'_>> = Vec::with_capacity(2);
        slot_buf.push(
            TreeIconSlot::new(TreeIconKind::Eye, &mut visible_flag)
                .with_tooltip("Toggle visibility"),
        );
        if let Some(c) = swatch {
            slot_buf.push(TreeIconSlot::new(TreeIconKind::Color(c), &mut color_sentinel));
        }

        if has_children {
            let is_open = *expanded.0.entry(path_key.clone()).or_insert(true);
            let mut open_ref = is_open;
            let r = tree_row(
                ui,
                row_id_salt,
                depth,
                Some(&mut open_ref),
                None,
                &label_owned,
                is_selected,
                accent,
                &mut slot_buf,
            );
            if open_ref != is_open {
                expanded.0.insert(path_key.clone(), open_ref);
            }
            r
        } else {
            tree_row(
                ui,
                row_id_salt,
                depth,
                None,
                None,
                &label_owned,
                is_selected,
                accent,
                &mut slot_buf,
            )
        }
    };

    // Write the eye state back to the cache; the panel commits it
    // to the ECS after all rows have rendered.
    vis_cache.insert(entity, visible_flag);

    if resp.body.hovered() {
        resp.body.clone().on_hover_text(&prim_ref.path);
    }
    if resp.body.double_clicked() {
        outcome.double_clicked = Some(entity);
    } else if resp.body.clicked() {
        outcome.clicked = Some(entity);
    }

    context_menu_frost(&resp.body, accent, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        if wide_button(ui, "Fly to", accent).clicked() {
            outcome.ctx_action = Some(CtxAction::FlyTo(entity));
            ui.close();
        }
        if wide_button(ui, "Fit to bounds", accent).clicked() {
            outcome.ctx_action = Some(CtxAction::Fit(entity));
            ui.close();
        }
        if wide_button(ui, "Copy path", accent).clicked() {
            ui.ctx().copy_text(prim_ref.path.clone());
            ui.close();
        }
        if wide_button(ui, "Expand descendants", accent).clicked() {
            outcome.ctx_action = Some(CtxAction::ExpandDesc(entity));
            ui.close();
        }
        if wide_button(ui, "Collapse descendants", accent).clicked() {
            outcome.ctx_action = Some(CtxAction::CollapseDesc(entity));
            ui.close();
        }
    });

    let show_children = if has_children {
        *expanded.0.get(&path_key).unwrap_or(&true)
    } else {
        false
    };
    if show_children {
        for (child_entity, child_name, child_ref, child_dn) in prim_children {
            let sub = draw_tree_row(
                ui,
                child_entity,
                child_name,
                child_ref,
                child_dn,
                prims,
                mat_q,
                materials,
                vis_cache,
                children,
                selected,
                expanded,
                accent,
                depth + 1,
                false,
            );
            outcome.merge(sub);
        }
    }

    outcome
}

/// Walk the subtree rooted at `root`, transforming each descendant's
/// authored local extent into world space, and fold into one AABB.
/// Returns `(focus, distance)` sized for arcball framing. When no
/// descendant carries `UsdLocalExtent`, falls back to a heuristic.
fn fit_params_for_entity(
    root: Entity,
    gt_q: &Query<&GlobalTransform>,
    extent_q: &Query<&bevy_openusd::UsdLocalExtent>,
    children: &Query<&Children>,
    current_cam_dist: f32,
) -> (Vec3, f32) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut found = false;

    let mut stack: Vec<Entity> = vec![root];
    while let Some(e) = stack.pop() {
        if let (Ok(gt), Ok(le)) = (gt_q.get(e), extent_q.get(e)) {
            let m = gt.to_matrix();
            for i in 0..8 {
                let c = Vec3::new(
                    if i & 1 == 0 { le.min[0] } else { le.max[0] },
                    if i & 2 == 0 { le.min[1] } else { le.max[1] },
                    if i & 4 == 0 { le.min[2] } else { le.max[2] },
                );
                let w = m.transform_point3(c);
                min = min.min(w);
                max = max.max(w);
            }
            found = true;
        }
        if let Ok(cs) = children.get(e) {
            for c in cs.iter() {
                stack.push(c);
            }
        }
    }

    if found {
        let center = (min + max) * 0.5;
        let size = (max - min).abs();
        let max_dim = size.x.max(size.y).max(size.z).max(0.05);
        // 1.6× the biggest dimension: fits the subtree with a small
        // margin, regardless of aspect ratio.
        let dist = (max_dim * 1.6).clamp(0.2, 200.0);
        (center, dist)
    } else if let Ok(gt) = gt_q.get(root) {
        // No local extent on anything in the subtree — fall back to
        // the single-click heuristic.
        (gt.translation(), (current_cam_dist * 0.25).clamp(0.2, 40.0))
    } else {
        (Vec3::ZERO, current_cam_dist)
    }
}

// ─── Stage-info panel ───────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_info_panel(
    mut contexts: EguiContexts,
    open: Res<RibbonOpen>,
    placement: Res<RibbonPlacement>,
    accent: Res<AccentColor>,
    info: Res<StageInfo>,
    mut reload: ResMut<ReloadRequest>,
    prims: Query<&UsdPrimRef>,
    meshes_q: Query<&Mesh3d, With<UsdPrimRef>>,
    spatial_audio_q: Query<&UsdSpatialAudio>,
    procedural_q: Query<&UsdProcedural>,
) {
    if !is_panel_open(&open, RIB_INFO) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let accent_col = accent.0;
    let mut keep = true;
    floating_window_for_item(
        ctx,
        RIBBONS,
        RIBBON_ITEMS,
        &placement,
        RIB_INFO,
        "Stage info",
        egui::vec2(PANEL_W, PANEL_H + 40.0),
        &mut keep,
        accent_col,
        |pane| {
            pane.section("info_stage", "Stage", true, |ui| {
                readout_row(ui, "file", &info.path);
                readout_row(
                    ui,
                    "defaultPrim",
                    info.default_prim.as_deref().unwrap_or("—"),
                );
                readout_row(ui, "layers", &info.layer_count.to_string());
                readout_row(ui, "prims", &prims.iter().count().to_string());
                readout_row(ui, "meshes", &meshes_q.iter().count().to_string());
                readout_row(ui, "variants", &info.variant_count.to_string());
            });
            pane.section("info_lights", "Lights & instances", true, |ui| {
                let light_labels = [
                    format!("{} dir", info.lights_directional),
                    format!("{} pt", info.lights_point),
                    format!("{} spot", info.lights_spot),
                    format!("{} dome", info.lights_dome),
                ];
                let refs: Vec<&str> = light_labels.iter().map(String::as_str).collect();
                badge_row(ui, "lights", &refs, accent_col);

                let inst_labels = [
                    format!("{} prim", info.instance_prim_count),
                    format!("{} reuse", info.instance_prototype_reuses),
                ];
                let refs: Vec<&str> = inst_labels.iter().map(String::as_str).collect();
                badge_row(ui, "instances", &refs, accent_col);

                readout_row(
                    ui,
                    "animated",
                    &format!("{} prim(s)", info.animated_prim_count),
                );
            });
            pane.section("info_skel_render", "Skel & render", true, |ui| {
                let skel_labels = [
                    format!("{} skel", info.skeleton_count),
                    format!("{} root", info.skel_root_count),
                    format!("{} bind", info.skel_binding_count),
                ];
                let refs: Vec<&str> = skel_labels.iter().map(String::as_str).collect();
                badge_row(ui, "skel", &refs, accent_col);

                let render_labels = [
                    format!("{} settings", info.render_settings_count),
                    format!("{} product", info.render_product_count),
                    format!("{} var", info.render_var_count),
                ];
                let refs: Vec<&str> = render_labels.iter().map(String::as_str).collect();
                badge_row(ui, "render", &refs, accent_col);

                if let Some([w, h]) = info.render_primary_resolution {
                    readout_row(ui, "resolution", &format!("{w} × {h}"));
                }

                let phys_labels = [
                    format!("{} scene", info.physics_scene_count),
                    format!("{} rigid", info.rigid_body_count),
                    format!("{} joint", info.joint_count),
                ];
                let refs: Vec<&str> = phys_labels.iter().map(String::as_str).collect();
                badge_row(ui, "physics", &refs, accent_col);
            });
            pane.section("info_authoring", "Authoring detail", true, |ui| {
                readout_row(
                    ui,
                    "custom",
                    &format!(
                        "{} prim · {} layer entries",
                        info.custom_attr_prim_count, info.custom_layer_data_entries
                    ),
                );
                readout_row(
                    ui,
                    "subdiv",
                    &format!("{} mesh(es) subdivision", info.subdivision_prim_count),
                );
                readout_row(
                    ui,
                    "light-link",
                    &format!("{} light(s) linked", info.light_linked_count),
                );
                readout_row(
                    ui,
                    "clips",
                    &format!("{} prim(s) UsdClipsAPI", info.clip_prim_count),
                );
                readout_row(
                    ui,
                    "spatial-audio",
                    &format!("{} source(s)", spatial_audio_q.iter().count()),
                );
                readout_row(
                    ui,
                    "procedural",
                    &format!("{} prim(s)", procedural_q.iter().count()),
                );
            });
            pane.section("info_actions", "Actions", true, |ui| {
                if wide_button(ui, "⟳  Reload stage (R)", accent_col).clicked() {
                    reload.requested = true;
                }
            });
        },
    );
}

// ─── Variants panel ─────────────────────────────────────────────────

fn draw_variants_panel(
    mut contexts: EguiContexts,
    open: Res<RibbonOpen>,
    placement: Res<RibbonPlacement>,
    accent: Res<AccentColor>,
    usd_assets: Res<Assets<UsdAsset>>,
    mut loader_tuning: ResMut<LoaderTuning>,
    mut reload: ResMut<ReloadRequest>,
) {
    if !is_panel_open(&open, RIB_VARIANTS) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let accent_col = accent.0;
    let mut keep = true;
    floating_window_for_item(
        ctx,
        RIBBONS,
        RIBBON_ITEMS,
        &placement,
        RIB_VARIANTS,
        "Variants",
        egui::vec2(PANEL_W, PANEL_H),
        &mut keep,
        accent_col,
        |pane| {
            pane.section("variants_all", "Variant sets", true, |ui| {
                let asset = usd_assets.iter().next().map(|(_, a)| a);
                match asset {
                    Some(asset) if !asset.variants.is_empty() => {
                        sub_caption(
                            ui,
                            &format!("{} prims author variant sets", asset.variants.len()),
                        );
                        ui.add_space(style::space::BLOCK);

                        let mut changed = false;
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let mut entries: Vec<_> = asset.variants.iter().collect();
                            entries.sort_by(|a, b| a.0.cmp(b.0));
                            for (prim_path, sets) in entries {
                                nested_section(
                                    ui,
                                    prim_path.as_str(),
                                    prim_path.as_str(),
                                    accent_col,
                                    true,
                                    |ui| {
                                        for set in sets {
                                            let key = (prim_path.clone(), set.name.clone());
                                            let authored =
                                                set.selection.as_deref().unwrap_or("");
                                            let current = loader_tuning
                                                .variants
                                                .get(&key)
                                                .cloned()
                                                .unwrap_or_else(|| authored.to_string());

                                            if set.options.is_empty() {
                                                readout_row(ui, &set.name, "(no options)");
                                                continue;
                                            }

                                            let mut selected_idx = set
                                                .options
                                                .iter()
                                                .position(|o| o == &current)
                                                .unwrap_or(0);
                                            let options_str: Vec<&str> =
                                                set.options.iter().map(|s| s.as_str()).collect();

                                            labelled_row(ui, &set.name, |ui| {
                                                let r = dropdown_control(
                                                    ui,
                                                    (prim_path.as_str(), set.name.as_str()),
                                                    &mut selected_idx,
                                                    &options_str,
                                                    accent_col,
                                                );
                                                if r.changed() {
                                                    let picked =
                                                        set.options[selected_idx].clone();
                                                    if picked != current {
                                                        loader_tuning
                                                            .variants
                                                            .insert(key.clone(), picked);
                                                        changed = true;
                                                    }
                                                }
                                            });

                                            if !current.is_empty() && current != authored {
                                                labelled_row(ui, "", |ui| {
                                                    if ui
                                                        .small_button("reset to authored")
                                                        .clicked()
                                                    {
                                                        loader_tuning.variants.remove(&key);
                                                        changed = true;
                                                    }
                                                });
                                            }
                                        }
                                    },
                                );
                            }
                        });
                        if changed {
                            reload.requested = true;
                        }
                    }
                    Some(_) => {
                        sub_caption(ui, "Stage authors no variant sets.");
                    }
                    None => {
                        sub_caption(ui, "(no stage loaded yet)");
                    }
                }
            });
        },
    );
}

// ─── Cameras panel ──────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_cameras_panel(
    mut contexts: EguiContexts,
    open: Res<RibbonOpen>,
    placement: Res<RibbonPlacement>,
    accent: Res<AccentColor>,
    usd_assets: Res<Assets<UsdAsset>>,
    mut camera_mount: ResMut<CameraMount>,
    mut bookmarks: ResMut<CameraBookmarks>,
    mut fly: ResMut<FlyTo>,
    cameras: Query<&ArcballCamera>,
) {
    if !is_panel_open(&open, RIB_CAMERAS) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let accent_col = accent.0;
    let mut keep = true;
    floating_window_for_item(
        ctx,
        RIBBONS,
        RIBBON_ITEMS,
        &placement,
        RIB_CAMERAS,
        "Cameras",
        egui::vec2(PANEL_W, PANEL_H),
        &mut keep,
        accent_col,
        |pane| {
            pane.section("cameras_bookmarks", "Bookmarks", true, |ui| {
                if wide_button(ui, "💾  Save current view", accent_col).clicked() {
                    if let Ok(cam) = cameras.single() {
                        let seq = bookmarks.next_seq + 1;
                        bookmarks.next_seq = seq;
                        bookmarks.items.push(CameraBookmark {
                            name: format!("View {seq}"),
                            focus: cam.focus,
                            distance: cam.distance,
                            yaw: cam.yaw,
                            elevation: cam.elevation,
                        });
                    }
                }
                if bookmarks.items.is_empty() {
                    sub_caption(ui, "(no bookmarks yet)");
                } else {
                    let mut to_delete: Option<usize> = None;
                    let mut to_jump: Option<usize> = None;
                    for (idx, bm) in bookmarks.items.iter().enumerate() {
                        let r = hybrid_select_row(
                            ui,
                            ("bookmark", idx),
                            &bm.name,
                            Some(&format!("d {:.1}", bm.distance)),
                            false,
                            false,
                            accent_col,
                        );
                        if r.body.clicked() {
                            to_jump = Some(idx);
                        }
                        if r.radio.clicked() {
                            to_delete = Some(idx);
                        }
                    }
                    if let Some(idx) = to_jump
                        && let (Ok(cam), Some(bm)) =
                            (cameras.single(), bookmarks.items.get(idx))
                    {
                        *camera_mount = CameraMount::Arcball;
                        fly.start_focus = cam.focus;
                        fly.start_distance = cam.distance;
                        fly.start_yaw = Some(cam.yaw);
                        fly.start_elevation = Some(cam.elevation);
                        fly.target_focus = bm.focus;
                        fly.target_distance = bm.distance;
                        fly.target_yaw = Some(bm.yaw);
                        fly.target_elevation = Some(bm.elevation);
                        fly.duration = 0.5;
                        fly.remaining = 0.5;
                    }
                    if let Some(idx) = to_delete {
                        bookmarks.items.remove(idx);
                    }
                    sub_caption(ui, "Click row to jump · click radio to delete");
                }
            });

            pane.section("cameras_all", "Cameras", true, |ui| {
                let asset = usd_assets.iter().next().map(|(_, a)| a);
                let Some(asset) = asset else {
                    sub_caption(ui, "(no stage loaded yet)");
                    return;
                };
                sub_caption(ui, &format!("{} authored cameras", asset.cameras.len()));
                ui.add_space(style::space::BLOCK);

                let arcball_active = matches!(*camera_mount, CameraMount::Arcball);
                let r = hybrid_select_row(
                    ui,
                    "arcball_mount",
                    "🎮  Arcball (free)",
                    None,
                    arcball_active,
                    arcball_active,
                    accent_col,
                );
                if r.body.clicked() || r.radio.clicked() {
                    *camera_mount = CameraMount::Arcball;
                }

                row_separator(ui);

                egui::ScrollArea::vertical().show(ui, |ui| {
                    for cam in &asset.cameras {
                        let mounted = matches!(
                            &*camera_mount,
                            CameraMount::Mounted { prim_path } if prim_path == &cam.path
                        );
                        let name = cam.path.rsplit('/').next().unwrap_or(&cam.path);
                        let focal = cam.data.focal_length_mm.unwrap_or(50.0);
                        let proj = match cam.data.projection {
                            Some(usd_schemas::camera::Projection::Orthographic) => "ortho",
                            _ => "persp",
                        };
                        let label = format!("📷  {name}");
                        let trailing = format!("{focal:.0}mm · {proj}");
                        let r = hybrid_select_row(
                            ui,
                            cam.path.as_str(),
                            &label,
                            Some(&trailing),
                            mounted,
                            mounted,
                            accent_col,
                        );
                        if r.body.clicked() || r.radio.clicked() {
                            *camera_mount = CameraMount::Mounted {
                                prim_path: cam.path.clone(),
                            };
                        }
                    }
                });
            });
        },
    );
}

// ─── Overlays panel ─────────────────────────────────────────────────

fn draw_overlays_panel(
    mut contexts: EguiContexts,
    open: Res<RibbonOpen>,
    placement: Res<RibbonPlacement>,
    accent: Res<AccentColor>,
    mut toggles: ResMut<DisplayToggles>,
    mut loader_tuning: ResMut<LoaderTuning>,
) {
    if !is_panel_open(&open, RIB_OVERLAYS) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let accent_col = accent.0;
    let mut keep = true;
    floating_window_for_item(
        ctx,
        RIBBONS,
        RIBBON_ITEMS,
        &placement,
        RIB_OVERLAYS,
        "Overlays",
        egui::vec2(PANEL_W, PANEL_H),
        &mut keep,
        accent_col,
        |pane| {
            pane.section("overlay_toggles", "World overlays", true, |ui| {
                toggle(ui, "Ground grid (G)", &mut toggles.show_world_grid, accent_col);
                toggle(ui, "World axes (X)", &mut toggles.show_world_axes, accent_col);
                toggle(
                    ui,
                    "Prim markers (P)",
                    &mut toggles.show_prim_markers,
                    accent_col,
                );
                let mut v = toggles.prim_marker_bias as f64;
                if pretty_slider(ui, "Prim marker bias", &mut v, 0.0..=5.0, 2, "×", accent_col)
                    .changed()
                {
                    toggles.prim_marker_bias = v as f32;
                }
                toggle(
                    ui,
                    "Skeleton bones (B)",
                    &mut toggles.show_skeleton,
                    accent_col,
                );
            });

            pane.section("overlay_render", "Render", true, |ui| {
                toggle(ui, "Wireframe", &mut toggles.wireframe, accent_col);
                let mut s = toggles.light_intensity_scale as f64;
                if pretty_slider(
                    ui,
                    "Light intensity",
                    &mut s,
                    0.0..=5.0,
                    2,
                    "×",
                    accent_col,
                )
                .changed()
                {
                    toggles.light_intensity_scale = s as f32;
                }
                sub_caption(ui, "Scales every authored light from its original value.");
            });

            pane.section("overlay_curves", "Curves (tubes)", true, |ui| {
                sub_caption(ui, "Default radius used when widths aren't authored");
                let mut r = loader_tuning.curves.default_radius as f64;
                if pretty_slider(ui, "Radius", &mut r, 0.001..=0.2, 3, " m", accent_col)
                    .changed()
                {
                    loader_tuning.curves.default_radius = r as f32;
                }
                let mut seg = loader_tuning.curves.ring_segments as f64;
                if pretty_slider(ui, "Ring segments", &mut seg, 3.0..=24.0, 0, "", accent_col)
                    .changed()
                {
                    loader_tuning.curves.ring_segments = seg.round() as u32;
                }
                let mut ps = loader_tuning.curves.point_scale as f64;
                if pretty_slider(ui, "Point scale", &mut ps, 0.05..=4.0, 2, "×", accent_col)
                    .changed()
                {
                    loader_tuning.curves.point_scale = ps as f32;
                }
                sub_caption(ui, "Sliders apply live — no reload needed.");
            });
        },
    );
}

// ─── Timeline panel ─────────────────────────────────────────────────

fn draw_timeline_panel(
    mut contexts: EguiContexts,
    open: Res<RibbonOpen>,
    placement: Res<RibbonPlacement>,
    accent: Res<AccentColor>,
    mut clock: ResMut<UsdStageTime>,
    usd_assets: Res<Assets<UsdAsset>>,
) {
    if !is_panel_open(&open, RIB_TIMELINE) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let accent_col = accent.0;
    let mut keep = true;
    floating_window_for_item(
        ctx,
        RIBBONS,
        RIBBON_ITEMS,
        &placement,
        RIB_TIMELINE,
        "Timeline",
        egui::vec2(PANEL_W, 320.0),
        &mut keep,
        accent_col,
        |pane| {
            pane.section("timeline_playback", "Playback", true, |ui| {
                let asset = usd_assets.iter().next().map(|(_, a)| a);
                let animated_count = asset.map(|a| a.animated_prims.len()).unwrap_or(0);
                sub_caption(
                    ui,
                    &format!(
                        "{animated_count} animated prim(s) · {:.1} fps · {:.1}s total",
                        clock.time_codes_per_second,
                        clock.duration_seconds()
                    ),
                );
                ui.add_space(style::space::BLOCK);

                let play_label = if clock.playing { "⏸  Pause" } else { "▶  Play" };
                if wide_button(ui, play_label, accent_col).clicked() {
                    clock.playing = !clock.playing;
                }
                if wide_button(ui, "⏮  Rewind", accent_col).clicked() {
                    clock.seconds = 0.0;
                }

                ui.add_space(style::space::BLOCK);
                let dur = clock.duration_seconds().max(1e-3);
                let _ = pretty_slider(
                    ui,
                    "Seconds",
                    &mut clock.seconds,
                    0.0..=dur,
                    3,
                    " s",
                    accent_col,
                );

                readout_row(ui, "timeCode", &format!("{:.3}", clock.current_time_code()));
                readout_row(
                    ui,
                    "range",
                    &format!("{:.2} … {:.2}", clock.start_time_code, clock.end_time_code),
                );
                readout_row(ui, "fps", &format!("{:.2}", clock.time_codes_per_second));
            });
        },
    );
}

// ─── Keys panel ─────────────────────────────────────────────────────

fn draw_keys_panel(
    mut contexts: EguiContexts,
    open: Res<RibbonOpen>,
    placement: Res<RibbonPlacement>,
    accent: Res<AccentColor>,
) {
    if !is_panel_open(&open, RIB_KEYS) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let accent_col = accent.0;
    let mut keep = true;
    floating_window_for_item(
        ctx,
        RIBBONS,
        RIBBON_ITEMS,
        &placement,
        RIB_KEYS,
        "Controls",
        egui::vec2(PANEL_W, PANEL_H),
        &mut keep,
        accent_col,
        |pane| {
            pane.section("keys_camera", "Camera", true, |ui| {
                keybinding_row(ui, "L+R drag", "Orbit");
                keybinding_row(ui, "Middle", "Pan");
                keybinding_row(ui, "Scroll", "Zoom");
            });
            pane.section("keys_panels", "Panels", true, |ui| {
                keybinding_row(ui, "T", "Toggle prim tree");
                keybinding_row(ui, "I", "Toggle stage info");
                keybinding_row(ui, "O", "Toggle overlays");
                keybinding_row(ui, "?", "Toggle this panel");
            });
            pane.section("keys_overlays", "Overlays", true, |ui| {
                keybinding_row(ui, "G", "Ground grid");
                keybinding_row(ui, "X", "World axes");
                keybinding_row(ui, "P", "Prim markers");
                keybinding_row(ui, "B", "Skeleton bones");
            });
            pane.section("keys_stage", "Stage", true, |ui| {
                keybinding_row(ui, "R", "Reload stage from disk");
            });
        },
    );
    // Suppress unused warning when accent_col isn't read inside bodies.
    let _ = accent_col;
}

// ─── Status bar (bottom-right, always visible) ──────────────────────

fn draw_status_bar(
    mut contexts: EguiContexts,
    info: Res<StageInfo>,
    accent: Res<AccentColor>,
    prims: Query<&UsdPrimRef>,
    meshes_q: Query<&Mesh3d, With<UsdPrimRef>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let prim_count = prims.iter().count();
    let mesh_count = meshes_q.iter().count();
    let accent_col = accent.0;
    statusbar(
        ctx,
        "viewer_status_bar",
        egui::Align2::RIGHT_BOTTOM,
        accent_col,
        |ui| {
            ui.label(
                egui::RichText::new(&info.path)
                    .color(style::TEXT_PRIMARY)
                    .monospace()
                    .small(),
            );
            status_sep(ui);
            ui.label(
                egui::RichText::new(format!("{prim_count} prims"))
                    .color(style::TEXT_SECONDARY)
                    .small(),
            );
            status_sep(ui);
            ui.label(
                egui::RichText::new(format!("{mesh_count} meshes"))
                    .color(style::TEXT_SECONDARY)
                    .small(),
            );
            status_sep(ui);
            ui.label(
                egui::RichText::new(format!("{} layers", info.layer_count))
                    .color(style::TEXT_SECONDARY)
                    .small(),
            );
        },
    );
}

// ─── Log panel ──────────────────────────────────────────────────────

fn draw_log_panel(
    mut contexts: EguiContexts,
    open: Res<RibbonOpen>,
    placement: Res<RibbonPlacement>,
    accent: Res<AccentColor>,
    log: Res<crate::log_panel::LoaderLog>,
) {
    if !is_panel_open(&open, RIB_LOG) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let accent_col = accent.0;
    let mut keep = true;
    floating_window_for_item(
        ctx,
        RIBBONS,
        RIBBON_ITEMS,
        &placement,
        RIB_LOG,
        "Log",
        egui::vec2(PANEL_W + 80.0, PANEL_H),
        &mut keep,
        accent_col,
        |pane| {
            pane.section("log_lines", "Loader log", true, |ui| {
                let count = log.buffer.lock().map(|b| b.len()).unwrap_or(0);
                sub_caption(ui, &format!("{count} entries · capped at 500"));
                ui.horizontal(|ui| {
                    if ui.small_button("Clear").clicked()
                        && let Ok(mut buf) = log.buffer.lock()
                    {
                        buf.clear();
                    }
                });
                ui.add_space(style::space::TIGHT);

                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let snapshot: Vec<crate::log_panel::LogLine> = log
                            .buffer
                            .lock()
                            .map(|b| b.iter().cloned().collect())
                            .unwrap_or_default();
                        if snapshot.is_empty() {
                            sub_caption(ui, "(no events yet — load a stage)");
                            return;
                        }
                        for line in &snapshot {
                            let level_color = level_to_color(line.level);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.painter().rect_filled(
                                    egui::Rect::from_center_size(
                                        ui.cursor().min + egui::vec2(4.0, 8.0),
                                        egui::vec2(6.0, 6.0),
                                    ),
                                    egui::CornerRadius::same(1),
                                    level_color,
                                );
                                ui.add_space(10.0);
                                ui.label(
                                    egui::RichText::new(short_target(&line.target))
                                        .small()
                                        .monospace()
                                        .color(style::TEXT_SECONDARY),
                                );
                                ui.label(
                                    egui::RichText::new(&line.message)
                                        .small()
                                        .color(style::TEXT_PRIMARY),
                                );
                            });
                        }
                    });
            });
        },
    );
}

fn level_to_color(level: bevy::log::Level) -> egui::Color32 {
    match level {
        bevy::log::Level::ERROR => style::DANGER,
        bevy::log::Level::WARN => style::WARNING,
        bevy::log::Level::INFO => style::SUCCESS,
        _ => style::TEXT_SECONDARY,
    }
}

fn short_target(target: &str) -> String {
    // `bevy_openusd::asset` → `asset`. Drops the crate prefix so the
    // log row stays readable at panel width.
    target.rsplit("::").next().unwrap_or(target).to_string()
}

// ─── Command palette (Ctrl+K) ───────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_palette_panel(
    mut contexts: EguiContexts,
    accent: Res<AccentColor>,
    mut palette: ResMut<ViewerCommandPalette>,
    mut ribbon: ResMut<RibbonOpen>,
    mut toggles: ResMut<DisplayToggles>,
    mut reload: ResMut<ReloadRequest>,
    mut load_req: ResMut<LoadRequest>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let Some(id) = command_palette(ctx, &mut palette.0, PALETTE_ITEMS, accent.0) else {
        return;
    };
    match id {
        "open_selection" => {
            ribbon.per_ribbon.insert(RIBBON_LEFT, RIB_SELECTION);
        }
        "open_tree" => {
            ribbon.per_ribbon.insert(RIBBON_LEFT, RIB_TREE);
        }
        "open_info" => {
            ribbon.per_ribbon.insert(RIBBON_LEFT, RIB_INFO);
        }
        "open_variants" => {
            ribbon.per_ribbon.insert(RIBBON_LEFT, RIB_VARIANTS);
        }
        "open_cameras" => {
            ribbon.per_ribbon.insert(RIBBON_LEFT, RIB_CAMERAS);
        }
        "open_overlays" => {
            ribbon.per_ribbon.insert(RIBBON_LEFT, RIB_OVERLAYS);
        }
        "open_timeline" => {
            ribbon.per_ribbon.insert(RIBBON_LEFT, RIB_TIMELINE);
        }
        "open_keys" => {
            ribbon.per_ribbon.insert(RIBBON_LEFT, RIB_KEYS);
        }
        "open_log" => {
            ribbon.per_ribbon.insert(RIBBON_LEFT, RIB_LOG);
        }
        "toggle_grid" => {
            toggles.show_world_grid = !toggles.show_world_grid;
        }
        "toggle_axes" => {
            toggles.show_world_axes = !toggles.show_world_axes;
        }
        "toggle_markers" => {
            toggles.show_prim_markers = !toggles.show_prim_markers;
        }
        "toggle_wireframe" => {
            toggles.wireframe = !toggles.wireframe;
        }
        "reload_stage" => {
            reload.requested = true;
        }
        "browse_usd" => {
            if let Some(picked) = rfd::FileDialog::new()
                .add_filter("USD stages", &["usda", "usdc", "usd", "usdz"])
                .pick_file()
            {
                load_req.path = Some(PathBuf::from(picked));
            }
        }
        _ => {}
    }
    palette.0.open = false;
}

fn status_sep(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("·")
            .color(style::TEXT_SECONDARY)
            .small(),
    );
}
