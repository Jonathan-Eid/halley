//! Per-window snap geometry and cross-output cycling.
//!
//! This module is the pure model behind the `snap-left` / `snap-right` /
//! `snap-up` / `snap-down` actions. It knows nothing about Wayland, cameras,
//! or decorations: the session supplies one gap-inset work rectangle per
//! output in node (Field/logical) coordinates and receives the outer
//! rectangle for the target half back.
//!
//! The horizontal strip is every connected output sorted by logical x
//! offset. Each output contributes its left half then its right half, so the
//! strip for outputs at x = 0, 1920, 5383 is
//! `[HDMI-L, HDMI-R, DP-L, DP-R, eDP-L, eDP-R]`. Moving left decrements the
//! index (wrapping), moving right increments it. That is the guiding
//! principle in code: entering a new monitor lands on the half nearest the
//! previous position, then successive presses toggle halves inside that
//! monitor before leaving its far edge.
//!
//! Vertical snaps never cycle: they target the top or bottom half of the
//! output the window is already on.

use std::collections::HashMap;

use crate::field::NodeId;

/// Which half of an output work rectangle a snap targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Half {
    Left,
    Right,
    Top,
    Bottom,
}

/// Horizontal travel direction for strip cycling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Horizontal {
    Left,
    Right,
}

/// Vertical snap direction. No cycling across outputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vertical {
    Up,
    Down,
}

/// An axis-aligned rectangle in node coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl RectI {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(self) -> i32 {
        self.y + self.h
    }

    pub fn center_x(self) -> i32 {
        self.x + self.w / 2
    }

    pub fn center_y(self) -> i32 {
        self.y + self.h / 2
    }

    pub fn contains_x(self, x: i32) -> bool {
        x >= self.x && x < self.right()
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        self.contains_x(x) && y >= self.y && y < self.bottom()
    }

    pub fn translated(self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..self
        }
    }
}

/// One output's gap-inset work rectangle in node coordinates, plus the
/// logical x offset used for strip ordering. Callers supply
/// `sort_x` from the output's global logical position (see
/// `build_strip`); the work rectangle itself is already gap-inset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputWork {
    pub name: String,
    pub sort_x: i32,
    pub sort_y: i32,
    pub work: RectI,
}

/// One half of one output: a cell in the horizontal strip, or a vertical
/// target on a single output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapCell {
    pub output: String,
    pub half: Half,
    /// Outer rectangle in node coordinates. The session converts this to a
    /// client rectangle with decorations, as maximize and arrange do.
    pub rect: RectI,
}

/// The outer rectangle of one half of `work`. Halves partition `work`
/// exactly: left width is `w / 2`, right takes the remainder, so odd widths
/// give the extra pixel to the right half. The same rule applies
/// vertically. Degenerate work rectangles (smaller than 2 on the split
/// axis) yield a single-purpose cell covering `work` on that axis rather
/// than a zero-sized rect.
pub fn half_rect(work: RectI, half: Half) -> RectI {
    match half {
        Half::Left | Half::Right => {
            if work.w < 2 {
                return work;
            }
            let left_w = work.w / 2;
            if half == Half::Left {
                RectI::new(work.x, work.y, left_w, work.h)
            } else {
                RectI::new(work.x + left_w, work.y, work.w - left_w, work.h)
            }
        }
        Half::Top | Half::Bottom => {
            if work.h < 2 {
                return work;
            }
            let top_h = work.h / 2;
            if half == Half::Top {
                RectI::new(work.x, work.y, work.w, top_h)
            } else {
                RectI::new(work.x, work.y + top_h, work.w, work.h - top_h)
            }
        }
    }
}

/// Build the horizontal strip: outputs sorted by logical x (then y, then
/// name for determinism when two outputs share an x), each contributing its
/// left half then its right half.
pub fn build_strip(outputs: &[OutputWork]) -> Vec<SnapCell> {
    let mut sorted = outputs.to_vec();
    sorted.sort_by(|a, b| {
        a.sort_x
            .cmp(&b.sort_x)
            .then(a.sort_y.cmp(&b.sort_y))
            .then(a.name.cmp(&b.name))
    });
    let mut cells = Vec::with_capacity(sorted.len() * 2);
    for output in &sorted {
        for half in [Half::Left, Half::Right] {
            cells.push(SnapCell {
                output: output.name.clone(),
                half,
                rect: half_rect(output.work, half),
            });
        }
    }
    cells
}

/// The next strip index in the travel direction, wrapping at both ends.
/// Returns `None` for an empty strip.
pub fn next_index(current: usize, len: usize, direction: Horizontal) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let current = current % len;
    Some(match direction {
        Horizontal::Right => (current + 1) % len,
        Horizontal::Left => (current + len - 1) % len,
    })
}

/// Where the first horizontal snap lands for a window that is not currently
/// snapped, given its centre x in node coordinates.
///
/// The centre's output determines the starting monitor. A centre on the
/// right of that output's midline snaps left to that output's left half;
/// a centre left of the midline snaps left to the previous output's right
/// half (wrapping). Right is the mirror image. A centre exactly on the
/// midline (a centred window) therefore goes to the same output's half on
/// the pressed side, which is the natural first step.
pub fn initial_index(
    outputs: &[OutputWork],
    center_x: i32,
    direction: Horizontal,
) -> Option<usize> {
    if outputs.is_empty() {
        return None;
    }
    let mut sorted = outputs.to_vec();
    sorted.sort_by(|a, b| {
        a.sort_x
            .cmp(&b.sort_x)
            .then(a.sort_y.cmp(&b.sort_y))
            .then(a.name.cmp(&b.name))
    });
    // Index of the strip cell for (`output_pos`, `half`): two cells per
    // output, left first.
    let cell = |output_pos: usize, half: Half| -> usize {
        output_pos * 2 + usize::from(half == Half::Right)
    };
    // Find the output whose work rectangle contains the centre, else the
    // nearest output by distance to its x interval.
    let mut best_pos = 0usize;
    let mut best_dist = i64::MAX;
    for (pos, output) in sorted.iter().enumerate() {
        let dist = if center_x < output.work.x {
            i64::from(output.work.x - center_x)
        } else if center_x >= output.work.right() {
            i64::from(center_x - output.work.right() + 1)
        } else {
            0
        };
        if dist < best_dist {
            best_dist = dist;
            best_pos = pos;
            if dist == 0 {
                break;
            }
        }
    }
    let output = &sorted[best_pos];
    let mid = output.work.x + output.work.w / 2;
    let len = sorted.len() * 2;
    let idx = match direction {
        Horizontal::Left => {
            if center_x >= mid {
                cell(best_pos, Half::Left)
            } else {
                // Previous output's right half, wrapping to the last output.
                let prev = (best_pos + sorted.len() - 1) % sorted.len();
                cell(prev, Half::Right)
            }
        }
        Horizontal::Right => {
            if center_x <= mid {
                cell(best_pos, Half::Right)
            } else {
                let next = (best_pos + 1) % sorted.len();
                cell(next, Half::Left)
            }
        }
    };
    (idx < len).then_some(idx)
}

/// The vertical target for a window on `output`: top or bottom half of that
/// output's work rectangle only. No cycling, no strip.
pub fn vertical_cell(output: &OutputWork, direction: Vertical) -> SnapCell {
    let half = match direction {
        Vertical::Up => Half::Top,
        Vertical::Down => Half::Bottom,
    };
    SnapCell {
        output: output.name.clone(),
        half,
        rect: half_rect(output.work, half),
    }
}

/// Which stored cell a node is currently snapped to, and the geometry it
/// had before the first snap in the current snap sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapEntry {
    pub cell: SnapCell,
    /// The client geometry to restore on unsnap, in node coordinates.
    pub prior: RectI,
    pub prior_output: String,
}

/// Per-window snap state: `NodeId -> SnapEntry`.
#[derive(Clone, Debug, Default)]
pub struct SnapStore {
    entries: HashMap<NodeId, SnapEntry>,
}

impl SnapStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: NodeId) -> Option<&SnapEntry> {
        self.entries.get(&id)
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.entries.contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Record a snap. The first snap in a sequence stores `prior`; later
    /// cycle steps keep the original `prior` so unsnapping always returns
    /// to the pre-snap geometry, not to an intermediate snapped rect.
    pub fn insert(&mut self, id: NodeId, cell: SnapCell, prior: RectI, prior_output: String) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.cell = cell;
        } else {
            self.entries.insert(
                id,
                SnapEntry {
                    cell,
                    prior,
                    prior_output,
                },
            );
        }
    }

    /// Forget one window's snap state without restoring. Used when the
    /// window is closed or deliberately edited: the window stays where the
    /// user put it (the same rule as arrange invalidation).
    pub fn remove(&mut self, id: NodeId) -> bool {
        self.entries.remove(&id).is_some()
    }

    /// Unsnap bookkeeping: take the entry so the caller can restore its
    /// `prior` geometry. Returns `None` when the window is not snapped.
    pub fn take(&mut self, id: NodeId) -> Option<SnapEntry> {
        self.entries.remove(&id)
    }

    /// Forget snaps by surface identity is the session's job (it owns
    /// surfaces); this helper forgets by node id after a lookup.
    pub fn invalidate(&mut self, id: NodeId) -> bool {
        self.remove(id)
    }

    /// Shift stored geometry when an output's layout position changes, so
    /// snapped and prior rectangles stay attached to their monitor (the
    /// same rule as arrange restore translation).
    pub fn translate_output(&mut self, output: &str, dx: i32, dy: i32) {
        for entry in self.entries.values_mut() {
            if entry.cell.output == output {
                entry.cell.rect = entry.cell.rect.translated(dx, dy);
            }
            if entry.prior_output == output {
                entry.prior = entry.prior.translated(dx, dy);
            }
        }
    }

    /// Drop entries whose output is gone (output removed from the strip).
    pub fn retain_outputs(&mut self, outputs: &[String]) {
        self.entries.retain(|_, entry| {
            outputs.contains(&entry.cell.output) || outputs.contains(&entry.prior_output)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(name: &str, x: i32) -> OutputWork {
        OutputWork {
            name: name.to_string(),
            sort_x: x,
            sort_y: 0,
            work: RectI::new(x, 0, 1920, 1080),
        }
    }

    fn three_outputs() -> Vec<OutputWork> {
        vec![work("HDMI-A-1", 0), work("DP-4", 1920), work("eDP-1", 5383)]
    }

    fn label(cell: &SnapCell) -> String {
        format!("{}-{:?}", cell.output, cell.half)
    }

    #[test]
    fn strip_is_sorted_by_logical_x_with_left_then_right_per_output() {
        // Deliberately unsorted input: build_strip must sort.
        let outputs = vec![work("eDP-1", 5383), work("HDMI-A-1", 0), work("DP-4", 1920)];
        let strip = build_strip(&outputs);
        let names: Vec<String> = strip.iter().map(label).collect();
        assert_eq!(
            names,
            [
                "HDMI-A-1-Left",
                "HDMI-A-1-Right",
                "DP-4-Left",
                "DP-4-Right",
                "eDP-1-Left",
                "eDP-1-Right",
            ]
        );
        // Halves partition the work rectangle exactly.
        assert_eq!(strip[0].rect, RectI::new(0, 0, 960, 1080));
        assert_eq!(strip[1].rect, RectI::new(960, 0, 960, 1080));
        assert_eq!(strip[2].rect, RectI::new(1920, 0, 960, 1080));
        assert_eq!(strip[3].rect, RectI::new(2880, 0, 960, 1080));
    }

    #[test]
    fn left_from_dp4_centre_walks_the_task_example_and_wraps() {
        let outputs = three_outputs();
        let strip = build_strip(&outputs);
        assert_eq!(strip.len(), 6);
        // DP-4 centre: x = 1920 + 960 = 2880, exactly the midline.
        let start = initial_index(&outputs, 2880, Horizontal::Left).unwrap();
        assert_eq!(label(&strip[start]), "DP-4-Left");
        let mut idx = start;
        let mut seen = Vec::new();
        for _ in 0..6 {
            idx = next_index(idx, strip.len(), Horizontal::Left).unwrap();
            seen.push(label(&strip[idx]));
        }
        // HDMI right, HDMI left, wrap to eDP right, eDP left, DP-4 right,
        // then back to DP-4 left.
        assert_eq!(
            seen,
            [
                "HDMI-A-1-Right",
                "HDMI-A-1-Left",
                "eDP-1-Right",
                "eDP-1-Left",
                "DP-4-Right",
                "DP-4-Left",
            ]
        );
    }

    #[test]
    fn right_from_dp4_centre_is_the_mirror_walk() {
        let outputs = three_outputs();
        let strip = build_strip(&outputs);
        let start = initial_index(&outputs, 2880, Horizontal::Right).unwrap();
        assert_eq!(label(&strip[start]), "DP-4-Right");
        let mut idx = start;
        let mut seen = Vec::new();
        for _ in 0..6 {
            idx = next_index(idx, strip.len(), Horizontal::Right).unwrap();
            seen.push(label(&strip[idx]));
        }
        assert_eq!(
            seen,
            [
                "eDP-1-Left",
                "eDP-1-Right",
                "HDMI-A-1-Left",
                "HDMI-A-1-Right",
                "DP-4-Left",
                "DP-4-Right",
            ]
        );
    }

    #[test]
    fn within_monitor_toggling_precedes_leaving_the_far_edge() {
        // Single output: left/right simply toggle its two halves.
        let outputs = vec![work("DP-1", 0)];
        let strip = build_strip(&outputs);
        assert_eq!(strip.len(), 2);
        let left = initial_index(&outputs, 960, Horizontal::Left).unwrap();
        assert_eq!(label(&strip[left]), "DP-1-Left");
        let right = next_index(left, 2, Horizontal::Left).unwrap();
        assert_eq!(label(&strip[right]), "DP-1-Right");
        let left_again = next_index(right, 2, Horizontal::Left).unwrap();
        assert_eq!(left_again, left);
        // Off-centre starts: a window centred in the left half pressing
        // left leaves for the previous output (here: wraps to right half
        // of the same single output's strip, the only other cell).
        let from_left_half = initial_index(&outputs, 100, Horizontal::Left).unwrap();
        assert_eq!(label(&strip[from_left_half]), "DP-1-Right");
        let from_right_half = initial_index(&outputs, 1800, Horizontal::Right).unwrap();
        assert_eq!(label(&strip[from_right_half]), "DP-1-Left");
    }

    #[test]
    fn wrap_at_both_strip_ends() {
        assert_eq!(next_index(0, 6, Horizontal::Left), Some(5));
        assert_eq!(next_index(5, 6, Horizontal::Right), Some(0));
        assert_eq!(next_index(0, 0, Horizontal::Left), None);
        assert_eq!(initial_index(&[], 0, Horizontal::Right), None);
    }

    #[test]
    fn vertical_targets_stay_on_the_current_output() {
        let outputs = three_outputs();
        let dp4 = &outputs[1];
        let top = vertical_cell(dp4, Vertical::Up);
        assert_eq!(top.output, "DP-4");
        assert_eq!(top.half, Half::Top);
        assert_eq!(top.rect, RectI::new(1920, 0, 1920, 540));
        let bottom = vertical_cell(dp4, Vertical::Down);
        assert_eq!(bottom.rect, RectI::new(1920, 540, 1920, 540));
    }

    #[test]
    fn halves_handle_odd_sizes_and_gap_inset_work() {
        // Gap-inset work: 1920 - 2*20 = 1880 wide at x=20.
        let work = RectI::new(20, 20, 1880, 1040);
        assert_eq!(half_rect(work, Half::Left), RectI::new(20, 20, 940, 1040));
        assert_eq!(half_rect(work, Half::Right), RectI::new(960, 20, 940, 1040));
        // Odd width: extra pixel goes right.
        let odd = RectI::new(0, 0, 1919, 1079);
        assert_eq!(half_rect(odd, Half::Left).w, 959);
        assert_eq!(half_rect(odd, Half::Right).w, 960);
        assert_eq!(half_rect(odd, Half::Top).h, 539);
        assert_eq!(half_rect(odd, Half::Bottom).h, 540);
    }

    #[test]
    fn snap_store_keeps_the_original_prior_across_cycles() {
        let mut store = SnapStore::new();
        let id = NodeId::new(7);
        let prior = RectI::new(100, 100, 800, 600);
        let cell_a = SnapCell {
            output: "DP-4".into(),
            half: Half::Left,
            rect: RectI::new(1920, 0, 960, 1080),
        };
        let cell_b = SnapCell {
            output: "HDMI-A-1".into(),
            half: Half::Right,
            rect: RectI::new(960, 0, 960, 1080),
        };
        store.insert(id, cell_a.clone(), prior, "DP-4".into());
        store.insert(id, cell_b.clone(), prior, "DP-4".into());
        let entry = store.get(id).unwrap();
        assert_eq!(entry.cell, cell_b);
        assert_eq!(entry.prior, prior);
        assert_eq!(entry.prior_output, "DP-4");
        assert_eq!(store.len(), 1);
        // Unsnap takes the entry and restores the pre-snap rect.
        let taken = store.take(id).unwrap();
        assert_eq!(taken.prior, prior);
        assert!(!store.contains(id));
        assert!(store.take(id).is_none());
    }

    #[test]
    fn snap_store_invalidate_and_translate() {
        let mut store = SnapStore::new();
        let a = NodeId::new(1);
        let b = NodeId::new(2);
        let cell = SnapCell {
            output: "DP-1".into(),
            half: Half::Left,
            rect: RectI::new(0, 0, 960, 1080),
        };
        store.insert(a, cell.clone(), RectI::new(10, 10, 400, 300), "DP-1".into());
        store.insert(b, cell.clone(), RectI::new(20, 20, 400, 300), "DP-1".into());
        assert!(store.invalidate(a));
        assert!(!store.contains(a));
        assert!(store.contains(b));
        store.translate_output("DP-1", 100, 50);
        let entry = store.get(b).unwrap();
        assert_eq!(entry.cell.rect, RectI::new(100, 50, 960, 1080));
        assert_eq!(entry.prior, RectI::new(120, 70, 400, 300));
        store.translate_output("DP-2", 999, 999);
        assert_eq!(store.get(b).unwrap().prior, RectI::new(120, 70, 400, 300));
    }
}
