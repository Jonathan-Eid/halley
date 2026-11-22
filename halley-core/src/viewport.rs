use std::collections::HashMap;

use crate::field::{Rect, Vec2};

/// One output's projection of a shared Field, independent of the output's
/// desktop layout position. The caller converts desktop coordinates to/from
/// output-local coordinates at the display boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldProjection {
    world_center: Vec2,
    output_size: Vec2,
    zoom: f32,
}

impl FieldProjection {
    /// `output_size` is in output-local logical units, and `zoom` is the
    /// number of those units per Field unit.
    pub fn new(world_center: Vec2, output_size: Vec2, zoom: f32) -> Self {
        Self {
            world_center,
            output_size,
            zoom: zoom.max(f32::EPSILON),
        }
    }

    pub fn viewport(self) -> Viewport {
        Viewport::new(
            self.world_center,
            Vec2 {
                x: self.output_size.x / self.zoom,
                y: self.output_size.y / self.zoom,
            },
        )
    }

    pub fn output_from_world(self, world: Vec2) -> Vec2 {
        Vec2 {
            x: (world.x - self.world_center.x) * self.zoom + self.output_size.x * 0.5,
            y: (world.y - self.world_center.y) * self.zoom + self.output_size.y * 0.5,
        }
    }

    pub fn world_from_output(self, local: Vec2) -> Vec2 {
        Vec2 {
            x: (local.x - self.output_size.x * 0.5) / self.zoom + self.world_center.x,
            y: (local.y - self.output_size.y * 0.5) / self.zoom + self.world_center.y,
        }
    }
}

/// Which views may show a Field footprint. An output-local presentation must
/// only consider its own output, even if another camera sees the same region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldViewScope<'a> {
    Output(&'a str),
    Shared,
}

impl FieldViewScope<'_> {
    /// Tests geometric visibility in the supplied views. The caller supplies
    /// only outputs that can currently present the footprint; this does not
    /// test occlusion, output power, or workspace state.
    pub fn shows(self, views: &HashMap<String, Viewport>, pos: Vec2, footprint: Vec2) -> bool {
        match self {
            Self::Output(name) => views
                .get(name)
                .is_some_and(|view| view.shows(pos, footprint)),
            Self::Shared => views.values().any(|view| view.shows(pos, footprint)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Center position in Field coordinates.
    pub center: Vec2,

    /// Size of the visible region in Field coordinates.
    pub size: Vec2,

    /// Home position for Return.
    pub home: Vec2,
}

impl Viewport {
    pub fn new(center: Vec2, size: Vec2) -> Self {
        Self {
            center,
            size,
            home: center,
        }
    }

    /// Axis-aligned view rectangle in Field space.
    pub fn rect(&self) -> Rect {
        let half = Vec2 {
            x: self.size.x * 0.5,
            y: self.size.y * 0.5,
        };

        Rect {
            min: Vec2 {
                x: self.center.x - half.x,
                y: self.center.y - half.y,
            },
            max: Vec2 {
                x: self.center.x + half.x,
                y: self.center.y + half.y,
            },
        }
    }

    /// Move camera to a new center.
    pub fn move_to(&mut self, center: Vec2) {
        self.center = center;
    }

    /// Offset camera by delta.
    pub fn pan(&mut self, delta: Vec2) {
        self.center.x += delta.x;
        self.center.y += delta.y;
    }

    /// Set current position as home.
    pub fn set_home(&mut self) {
        self.home = self.center;
    }

    /// Return to home position.
    pub fn return_home(&mut self) {
        self.center = self.home;
    }
}

impl Viewport {
    /// Whether a Field point lies within the visible rectangle.
    pub fn contains(&self, p: Vec2) -> bool {
        let rect = self.rect();
        p.x >= rect.min.x && p.x <= rect.max.x && p.y >= rect.min.y && p.y <= rect.max.y
    }

    /// Whether any part of a footprint centred on `pos` is visible. A
    /// footprint that only touches the edge shows nothing, so it is not.
    pub fn shows(&self, pos: Vec2, footprint: Vec2) -> bool {
        let view = self.rect();
        let half_w = footprint.x.abs() * 0.5;
        let half_h = footprint.y.abs() * 0.5;
        pos.x - half_w < view.max.x
            && pos.x + half_w > view.min.x
            && pos.y - half_h < view.max.y
            && pos.y + half_h > view.min.y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_visibility_checks_other_views_but_local_presentations_do_not() {
        let origin = Vec2 { x: 0.0, y: 0.0 };
        let view_size = Vec2 { x: 100.0, y: 100.0 };
        let footprint = Vec2 { x: 20.0, y: 20.0 };
        let views = HashMap::from([
            (
                "DP-1".into(),
                Viewport::new(Vec2 { x: 500.0, y: 0.0 }, view_size),
            ),
            ("DP-2".into(), Viewport::new(origin, view_size)),
        ]);

        assert!(!FieldViewScope::Output("DP-1").shows(&views, origin, footprint));
        assert!(FieldViewScope::Output("DP-2").shows(&views, origin, footprint));
        assert!(!FieldViewScope::Output("disconnected").shows(&views, origin, footprint));
        assert!(FieldViewScope::Shared.shows(&views, origin, footprint));
        assert!(!FieldViewScope::Shared.shows(&views, Vec2 { x: 60.0, y: 0.0 }, footprint));
        assert!(!FieldViewScope::Shared.shows(&HashMap::new(), origin, footprint));
    }

    #[test]
    fn one_field_position_projects_through_two_independent_cameras() {
        let window = Vec2 { x: -120.0, y: 50.0 };
        let first = FieldProjection::new(
            Vec2 { x: -100.0, y: 20.0 },
            Vec2 { x: 800.0, y: 600.0 },
            1.0,
        );
        let second = FieldProjection::new(
            Vec2 { x: 0.0, y: 10.0 },
            Vec2 {
                x: 1200.0,
                y: 800.0,
            },
            0.5,
        );

        assert_eq!(first.output_from_world(window), Vec2 { x: 380.0, y: 330.0 });
        assert_eq!(
            second.output_from_world(window),
            Vec2 { x: 540.0, y: 420.0 }
        );
        assert_eq!(
            second.world_from_output(second.output_from_world(window)),
            window
        );
        assert_eq!(
            second.viewport().size,
            Vec2 {
                x: 2400.0,
                y: 1600.0
            }
        );

        let panned =
            FieldProjection::new(Vec2 { x: 1500.0, y: 0.0 }, Vec2 { x: 800.0, y: 600.0 }, 1.0);
        let footprint = Vec2 { x: 100.0, y: 80.0 };
        assert!(first.viewport().shows(window, footprint));
        assert!(!panned.viewport().shows(window, footprint));
        assert!(second.viewport().shows(window, footprint));
    }

    #[test]
    fn rect_is_correct() {
        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });

        let r = vp.rect();

        assert_eq!(r.min, Vec2 { x: -50.0, y: -25.0 });
        assert_eq!(r.max, Vec2 { x: 50.0, y: 25.0 });
    }

    #[test]
    fn return_home_works() {
        let mut vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });

        vp.pan(Vec2 { x: 10.0, y: 5.0 });
        assert_eq!(vp.center, Vec2 { x: 10.0, y: 5.0 });

        vp.return_home();
        assert_eq!(vp.center, Vec2 { x: 0.0, y: 0.0 });
    }

    #[test]
    fn shows_any_visible_part_of_a_footprint() {
        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 50.0 });
        let size = Vec2 { x: 20.0, y: 10.0 };

        assert!(vp.shows(Vec2 { x: 0.0, y: 0.0 }, size));
        // Straddling the right edge, and one unit still on screen.
        assert!(vp.shows(Vec2 { x: 50.0, y: 0.0 }, size));
        assert!(vp.shows(Vec2 { x: 59.0, y: 0.0 }, size));
        // Touching the edge from outside, and well past it.
        assert!(!vp.shows(Vec2 { x: 60.0, y: 0.0 }, size));
        assert!(!vp.shows(Vec2 { x: 0.0, y: -500.0 }, size));
    }

    #[test]
    fn zoomed_view_shows_a_window_clipped_in_both_axes() {
        let view_size = Vec2 {
            x: 2560.0,
            y: 1440.0,
        };
        let origin = Vec2 { x: 0.0, y: 0.0 };
        let zoomed = FieldProjection::new(origin, view_size, 0.35).viewport();
        let native = FieldProjection::new(origin, view_size, 1.0).viewport();
        let corner = zoomed.rect().max;
        let footprint = Vec2 { x: 800.0, y: 600.0 };

        assert!(zoomed.shows(corner, footprint));
        assert!(!native.shows(corner, footprint));
    }
}
