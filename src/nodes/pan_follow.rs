//! Pan-follow: a window carried by an edge pan follows the camera onto the
//! adjacent display it scrolls into.
//!
//! The model here starts from the existing drag-pan behavior: a camera pan
//! moves only the view, and a window's canvas position and owning output
//! never change alongside it. During an edge pan
//! ([`tick_grabbed_window_edge_pan`](crate::session::input::tick_grabbed_window_edge_pan))
//! the dragged window is clamped to the scrolling viewport of the output the
//! drag started on, so the camera can carry it across the layout seam while
//! its node record still names the start output — stranded off-screen, or
//! later demoted by decay against the wrong monitor's camera.
//!
//! When `field.pan-follow-into-adjacent` is enabled, that same pan tick asks
//! [`maybe_pan_follow`] whether the grabbed, expanded window has ended up
//! mostly outside its owning output's live world viewport but mostly inside
//! an adjacent display's. If so, ownership moves to that display via the same
//! reattachment primitive monitor transfer uses —
//! [`set_collapsed_output`](super::session_ops::set_collapsed_output) — and
//! the live grab's edge-pan output is updated so later ticks clamp against
//! the new viewport.
//!
//! This deliberately does not call
//! [`transfer_window`](super::keyboard_navigation::transfer_window): that
//! command refuses to run while any compositor grab is active (it requires
//! `transfer_pointer_available`), and it recenters the window on the
//! destination camera and warps the pointer to the destination center — both
//! would fight the edge-pan drag, whose pointer must stay pinned at the
//! screen edge. Pan path over transfer command.
//!
//! Guard parity with transfer: cluster members, fullscreen or pending
//! fullscreen, maximized, user-pinned, and pointer-constrained windows never
//! follow. Destinations showing an open cluster workspace, or whose camera
//! is presentation-locked, are not candidates.

use smithay::utils::{Logical, Rectangle};

/// Strict majority test on an area: more than half of the window is visible
/// through the viewport. The comparison is strict so an exact 50/50 straddle
/// across the seam neither keeps nor takes ownership.
fn mostly_inside(inside: i64, area: i64) -> bool {
    inside * 2 > area
}

/// Picks the display a mostly-stranded dragged window should follow onto.
///
/// `window` is the window's live global rect; `owner_viewport` is the part of
/// global world space the owning output's camera currently shows; each
/// candidate is another display's live world viewport. Returns the
/// candidate's output name when the window is mostly outside the owner's
/// viewport but mostly inside one candidate's.
pub(super) fn pan_follow_target(
    window: Rectangle<i32, Logical>,
    owner_viewport: Rectangle<i32, Logical>,
    candidates: &[(String, Rectangle<i32, Logical>)],
) -> Option<String> {
    if window.size.w <= 0 || window.size.h <= 0 {
        return None;
    }
    let area = window.size.w as i64 * window.size.h as i64;
    let inside = |viewport: Rectangle<i32, Logical>| -> i64 {
        window
            .intersection(viewport)
            .map(|hit| hit.size.w as i64 * hit.size.h as i64)
            .unwrap_or(0)
    };
    // Still mostly owned: the window has not left the owner's view.
    if mostly_inside(inside(owner_viewport), area) {
        return None;
    }
    candidates
        .iter()
        .filter(|(_, viewport)| mostly_inside(inside(*viewport), area))
        .max_by_key(|(_, viewport)| inside(*viewport))
        .map(|(name, _)| name.clone())
}

/// Reassigns the edge-pan-grabbed window to the display it now mostly
/// overlaps, when the pan-follow option is on. Returns whether ownership
/// changed. Called from the edge-pan tick, which runs on both drivers.
pub(crate) fn maybe_pan_follow<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    id: halley_core::field::NodeId,
) -> bool {
    if !session.settings.field.pan_follow_into_adjacent {
        return false;
    }
    if crate::session::node_user_pinned(session, id) {
        return false;
    }
    let record = match session.nodes.record(id).cloned() {
        Some(record) => record,
        None => return false,
    };
    // Only an expanded, attached window follows. Collapsed nodes are carried
    // by different grabs and have no live client geometry worth tracking.
    if record.collapsed || !record.attached {
        return false;
    }
    // Same exclusions as `transfer_window`: cluster membership, fullscreen
    // (incl. pending), and Field maximize keep their own presentation.
    if session.clusters.cluster_for_member(id).is_some()
        || session
            .fullscreen
            .is_fullscreen_or_pending(&record.surface)
        || session.maximize.contains(&record.surface)
    {
        return false;
    }
    // A client holding a pointer lock/confinement owns pointer delivery;
    // reassigning its window mid-pan would fight it.
    if crate::session::has_active_pointer_constraint(session) {
        return false;
    }
    let window_rect = session
        .wayland
        .space
        .element_geometry(&record.window)
        .unwrap_or(record.geometry);
    let owner_viewport = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == record.output)
        .and_then(|output| {
            let geometry = session.wayland.space.output_geometry(output)?;
            let view = session.cameras.view(&output.name())?;
            Some(crate::presentation::camera::world_viewport(view, geometry))
        });
    let Some(owner_viewport) = owner_viewport else {
        return false;
    };
    let candidates = session
        .wayland
        .space
        .outputs()
        .filter(|output| output.name() != record.output)
        // The destination must show the Field, like `transfer_window`
        // requires: an open workspace or a presentation camera there cannot
        // host the window.
        .filter(|output| {
            session.clusters.active_on(&output.name()).is_none()
                && session.cameras.is_pannable(&output.name())
        })
        .filter_map(|output| {
            let geometry = session.wayland.space.output_geometry(output)?;
            let view = session.cameras.view(&output.name())?;
            Some((
                output.name(),
                crate::presentation::camera::world_viewport(view, geometry),
            ))
        })
        .collect::<Vec<_>>();
    let Some(target) = pan_follow_target(window_rect, owner_viewport, &candidates) else {
        return false;
    };
    let Some(destination) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == target)
        .cloned()
    else {
        return false;
    };
    session
        .interactions
        .field_arrange
        .invalidate_surface(&record.surface);
    super::session_ops::set_collapsed_output(session, id, &destination);
    // The live grab must clamp against the new output's viewport from the
    // next tick on, or it drags the window straight back into the old view.
    if let crate::input::grab::Grab::MoveWindow {
        edge_pan: Some(live),
        ..
    } = session.interactions.grab_mut()
    {
        live.output = target;
    }
    session.request_redraw();
    true
}

#[cfg(test)]
mod tests {
    use super::pan_follow_target;
    use smithay::utils::{Logical, Point, Rectangle, Size};

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((x, y)), Size::from((w, h)))
    }

    /// Two 1920x1080 displays tiled at the origin: left shows [-960, 960],
    /// right shows [960, 2880] once both cameras are at rest.
    fn side_by_side() -> (Rectangle<i32, Logical>, Vec<(String, Rectangle<i32, Logical>)>) {
        (
            rect(-960, -540, 1920, 1080),
            vec![("right".to_string(), rect(960, -540, 1920, 1080))],
        )
    }

    #[test]
    fn fully_inside_neighbor_follows() {
        let (owner, candidates) = side_by_side();
        let window = rect(1000, 0, 400, 300);
        assert_eq!(
            pan_follow_target(window, owner, &candidates),
            Some("right".to_string())
        );
    }

    #[test]
    fn mostly_inside_owner_stays() {
        let (owner, candidates) = side_by_side();
        let window = rect(0, 0, 400, 300);
        assert_eq!(pan_follow_target(window, owner, &candidates), None);
    }

    #[test]
    fn outside_both_viewports_stays() {
        let (owner, candidates) = side_by_side();
        // Below both displays entirely.
        let window = rect(0, 2000, 400, 300);
        assert_eq!(pan_follow_target(window, owner, &candidates), None);
    }

    #[test]
    fn exact_straddle_neither_keeps_nor_takes() {
        let (owner, candidates) = side_by_side();
        // 200px in each viewport of a 400px-wide window: 50/50 is neither
        // mostly outside (owner side) nor mostly inside (neighbor side).
        let window = rect(760, 0, 400, 300);
        assert_eq!(pan_follow_target(window, owner, &candidates), None);
    }

    #[test]
    fn majority_crossed_seam_follows() {
        let (owner, candidates) = side_by_side();
        // 60% in the neighbor, 40% still in the owner.
        let window = rect(800, 0, 400, 300);
        assert_eq!(
            pan_follow_target(window, owner, &candidates),
            Some("right".to_string())
        );
    }

    #[test]
    fn minority_crossing_stays() {
        let (owner, candidates) = side_by_side();
        // 40% in the neighbor, 60% still in the owner.
        let window = rect(720, 0, 400, 300);
        assert_eq!(pan_follow_target(window, owner, &candidates), None);
    }

    #[test]
    fn window_larger_than_neighbor_viewport_stays() {
        // Taller than the display: even fully overlapping, it can never be
        // mostly inside a viewport that is smaller than it.
        let (owner, candidates) = side_by_side();
        let window = rect(1000, -1000, 400, 3000);
        assert_eq!(pan_follow_target(window, owner, &candidates), None);
    }

    #[test]
    fn best_candidate_wins_when_several_match() {
        let owner = rect(-960, -540, 1920, 1080);
        // Both candidates carry a majority of the window; the display
        // carrying the most wins, not the first match.
        let candidates = vec![
            ("right".to_string(), rect(960, -540, 1920, 1080)),
            ("diag".to_string(), rect(900, 0, 1920, 1080)),
        ];
        let window = rect(600, 0, 800, 300);
        assert_eq!(
            pan_follow_target(window, owner, &candidates),
            Some("diag".to_string())
        );
    }

    #[test]
    fn no_candidates_means_no_follow() {
        let (owner, _) = side_by_side();
        let window = rect(1000, 0, 400, 300);
        assert_eq!(pan_follow_target(window, owner, &[]), None);
    }

    #[test]
    fn degenerate_window_never_follows() {
        let (owner, candidates) = side_by_side();
        let window = rect(1000, 0, 0, 300);
        assert_eq!(pan_follow_target(window, owner, &candidates), None);
    }
}
