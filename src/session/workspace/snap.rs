//! Per-window cross-output snap cycling (`snap-left` etc.).
//!
//! Field nodes only: collapsed nodes, cluster members, pinned windows, and
//! windows on an output owned by an active cluster workspace are excluded,
//! exactly like arrange and node movement. Snapping is an animated instant
//! cut (no slide): the window is configured to its target rect directly,
//! matching the other geometry actions. The pure cycle and geometry live in
//! `halley_core::snap`; this module is the session glue.
//!
//! Left/right cycle through every half of the full logical output strip in
//! the travel direction with wrap: outputs sorted by logical x, each
//! contributing left then right half. Entering a new monitor lands on the
//! half nearest the previous position, then presses toggle halves inside
//! that monitor before leaving its far edge. Up/down snap to the top or
//! bottom half of the current monitor only and never cycle across outputs.
//!
//! Geometry is the half of the gap-inset field work area (the visible work
//! area in node coordinates, as arrange computes it), converted from an
//! outer rect to a client rect with decorations, flush to the side.
//!
//! State: `field_snap` maps node id to the snapped cell plus the pre-snap
//! rect. The first snap in a sequence stores the prior rect; cycle steps
//! keep it, so unsnapping always returns to the pre-snap geometry. Up/down
//! toggling the same half again is the explicit unsnap: it restores the
//! prior rect and clears the entry. A stored entry whose snapped geometry
//! no longer matches the window (the user moved or resized it, it was
//! arranged or transferred) is treated as stale and discarded on the next
//! snap press, so manual edits implicitly unsnap without restoring — the
//! same rule as arrange invalidation.
//!
//! Fullscreen/maximize interplay: snapping a fullscreen or maximized window
//! first exits that presentation through the existing restore mechanics
//! (`set_record_fullscreen` / `toggle_field_maximize`, which restore the
//! pre-fullscreen/pre-maximize placement recorded by those managers), then
//! immediately configures the snap target as an instant cut. The snap prior
//! is seeded from that recorded pre-presentation placement when available,
//! so a later unsnap returns to the windowed geometry, not the fullscreen
//! rect. `toggle-fullscreen` itself already restores the pre-fullscreen
//! geometry: the fullscreen manager seeds `restore` from the live windowed
//! geometry on entry and cleanup drops the entry when the exit settles, so
//! re-entry re-seeds from the then-current geometry.

use halley_core::snap::{Horizontal, OutputWork, RectI, SnapCell, Vertical};
use smithay::utils::{Logical, Rectangle};

use super::{Session, SessionDriver};

fn rect_to_core(rect: Rectangle<i32, Logical>) -> RectI {
    RectI::new(rect.loc.x, rect.loc.y, rect.size.w, rect.size.h)
}

fn core_to_rect(rect: RectI) -> Rectangle<i32, Logical> {
    Rectangle::new((rect.x, rect.y).into(), (rect.w, rect.h).into())
}

fn rect_center(rect: Rectangle<i32, Logical>) -> (i32, i32) {
    (rect.loc.x + rect.size.w / 2, rect.loc.y + rect.size.h / 2)
}

/// One output's gap-inset work rectangle in node coordinates, plus its
/// global logical position for strip ordering.
fn output_work<D: SessionDriver>(
    session: &Session<D>,
    output_name: &str,
    gap: i32,
) -> Option<OutputWork> {
    let output = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)?
        .clone();
    let output_geometry = session.wayland.space.output_geometry(&output)?;
    let work_area = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
    // Visible work area in node coordinates (arrange's helper): this is
    // what makes a snapped window fill the half of the screen as seen,
    // under the current pan/zoom. Fall back to the global logical work
    // rect when no camera is available for the output.
    let world = session
        .cameras
        .get(output_name)
        .and_then(|camera| {
            super::arrange::visible_work_area_outer(camera, output_geometry, work_area, gap)
        })
        .map(|(outer, _)| outer)
        .unwrap_or_else(|| {
            let inset = gap.max(0);
            Rectangle::new(
                output_geometry.loc + work_area.loc + smithay::utils::Point::from((inset, inset)),
                (
                    work_area.size.w.saturating_sub(inset * 2).max(1),
                    work_area.size.h.saturating_sub(inset * 2).max(1),
                )
                    .into(),
            )
        });
    if world.size.w < 2 || world.size.h < 2 {
        return None;
    }
    Some(OutputWork {
        name: output_name.to_string(),
        sort_x: output_geometry.loc.x,
        sort_y: output_geometry.loc.y,
        work: rect_to_core(world),
    })
}

fn collect_works<D: SessionDriver>(session: &Session<D>, gap: i32) -> Vec<OutputWork> {
    let names = session
        .wayland
        .space
        .outputs()
        .map(|output| output.name())
        .collect::<Vec<_>>();
    names
        .iter()
        .filter_map(|name| output_work(session, name, gap))
        .collect()
}

pub(crate) fn snap_window<D: SessionDriver>(
    session: &mut Session<D>,
    direction: halley_config::Direction,
    output: Option<&str>,
) -> bool {
    let Some(record) = super::focused_window_record(session, output) else {
        return false;
    };
    let id = record.id;
    if session.clusters.is_member(id)
        || session.clusters.active_on(&record.output).is_some()
        || super::node_user_pinned(session, id)
        || crate::input::grab::belongs_to_surface(session.interactions.grab(), &record.surface)
    {
        return false;
    }
    let Some(live) = session.wayland.space.element_geometry(&record.window) else {
        return false;
    };
    let gap = session.settings.field.gap.ceil() as i32;
    let works = collect_works(session, gap);
    if works.is_empty() {
        return false;
    }
    let strip = halley_core::snap::build_strip(&works);
    if strip.is_empty() {
        return false;
    }
    let (center_x, _center_y) = rect_center(live);
    // Drop a stale entry: the window no longer sits where we snapped it.
    let live_entry = session
        .interactions
        .field_snap
        .get(id)
        .filter(|entry| {
            // Compare against the live outer rect: the stored cell rect is
            // an outer rect, the live geometry is the client rect. Convert
            // the stored outer to the client rect the same way the snap
            // was applied, then compare.
            let client = crate::titlebar::client_rect_for_outer(
                &record.window,
                core_to_rect(entry.cell.rect),
                &session.settings.decorations,
                &session.settings.font,
            );
            client == live
        })
        .cloned();

    // Vertical toggle: pressing the same vertical direction while already
    // snapped to that exact half unsnaps back to the pre-snap rect.
    if let Some(entry) = &live_entry {
        let same_vertical = matches!(
            (direction, entry.cell.half),
            (halley_config::Direction::Up, halley_core::snap::Half::Top)
                | (
                    halley_config::Direction::Down,
                    halley_core::snap::Half::Bottom
                )
        );
        if same_vertical {
            let entry = session
                .interactions
                .field_snap
                .take(id)
                .expect("live entry");
            super::configure_field_geometry(
                session,
                &crate::presentation::maximize::FieldRestore {
                    surface: record.surface.clone(),
                    geometry: core_to_rect(entry.prior),
                    output: entry.prior_output.clone(),
                },
            );
            session.request_redraw();
            return true;
        }
    }

    let target: SnapCell = match direction {
        halley_config::Direction::Left | halley_config::Direction::Right => {
            let horizontal = match direction {
                halley_config::Direction::Left => Horizontal::Left,
                _ => Horizontal::Right,
            };
            let index = match &live_entry {
                Some(entry) => {
                    // Continue the cycle from the stored cell, matched by
                    // identity in the current strip (the strip may have
                    // been rebuilt after an output change).
                    strip
                        .iter()
                        .position(|cell| {
                            cell.output == entry.cell.output && cell.half == entry.cell.half
                        })
                        .and_then(|pos| halley_core::snap::next_index(pos, strip.len(), horizontal))
                        .or_else(|| halley_core::snap::initial_index(&works, center_x, horizontal))
                }
                None => halley_core::snap::initial_index(&works, center_x, horizontal),
            };
            let Some(index) = index else {
                return false;
            };
            strip[index].clone()
        }
        halley_config::Direction::Up | halley_config::Direction::Down => {
            // Current monitor only: the output containing the window
            // centre, falling back to the record's output.
            let (cx, cy) = rect_center(live);
            let work = works
                .iter()
                .find(|work| work.work.contains(cx, cy))
                .or_else(|| works.iter().find(|work| work.name == record.output))
                .cloned();
            let Some(work) = work else {
                return false;
            };
            let vertical = match direction {
                halley_config::Direction::Up => Vertical::Up,
                _ => Vertical::Down,
            };
            halley_core::snap::vertical_cell(&work, vertical)
        }
    };

    // Prior rect: the original pre-snap geometry for this sequence, or the
    // live geometry — except fullscreen/maximized windows, whose live
    // geometry is the presentation rect. For those, seed from the recorded
    // pre-presentation placement so unsnap returns to the windowed rect.
    let prior = match &live_entry {
        Some(entry) => (entry.prior, entry.prior_output.clone()),
        None => {
            let seeded = session
                .fullscreen
                .restore_placement(&record.surface)
                .map(|(geometry, output)| {
                    (
                        rect_to_core(geometry),
                        output.unwrap_or_else(|| record.output.clone()),
                    )
                })
                .or_else(|| {
                    session
                        .maximize
                        .restore(&record.surface)
                        .map(|restore| (rect_to_core(restore.geometry), restore.output))
                });
            seeded.unwrap_or((rect_to_core(live), record.output.clone()))
        }
    };

    // Exit fullscreen/maximize through the existing restore mechanics
    // before configuring the snap target, so the snap cut is the last
    // writer to the window's geometry.
    if session.fullscreen.is_fullscreen_or_pending(&record.surface) {
        super::set_record_fullscreen(session, record.clone(), false);
    }
    if session.maximize.contains(&record.surface) {
        super::toggle_field_maximize(session, record.clone());
    }

    let target_client = crate::titlebar::client_rect_for_outer(
        &record.window,
        core_to_rect(target.rect),
        &session.settings.decorations,
        &session.settings.font,
    );
    session
        .interactions
        .field_snap
        .insert(id, target.clone(), prior.0, prior.1);
    super::configure_field_geometry(
        session,
        &crate::presentation::maximize::FieldRestore {
            surface: record.surface.clone(),
            geometry: target_client,
            output: target.output.clone(),
        },
    );
    pointer_reconcile(session);
    session.request_redraw();
    true
}

fn pointer_reconcile<D: SessionDriver>(session: &mut Session<D>) {
    crate::session::pointer::reconcile_state(session);
}

/// Forget snap state for a destroyed/closed window.
pub(crate) fn invalidate_surface<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) {
    if let Some(id) = session.nodes.id_for_surface(surface) {
        session.interactions.field_snap.invalidate(id);
    }
}
