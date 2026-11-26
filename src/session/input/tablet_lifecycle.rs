use std::collections::HashMap;
use std::hash::Hash;

pub(super) struct ProximityState<T, D = ()> {
    active: HashMap<T, D>,
}
impl<T, D> Default for ProximityState<T, D> {
    fn default() -> Self { Self { active: HashMap::new() } }
}
impl<T: Eq + Hash, D> ProximityState<T, D> {
    pub fn enter(&mut self, tool: T, device: D) { self.active.insert(tool, device); }
    pub fn contains(&self, tool: &T) -> bool { self.active.contains_key(tool) }
    pub fn leave(&mut self, tool: &T) -> bool { self.active.remove(tool).is_some() }
    pub fn cancel_all(&mut self) -> Vec<T> { self.active.drain().map(|(tool, _)| tool).collect() }
    pub fn cancel_device(&mut self, device: &D) -> Vec<T>
    where T: Clone, D: PartialEq {
        let tools: Vec<T> = self.active.iter().filter(|(_, owner)| *owner == device).map(|(tool, _)| tool.clone()).collect();
        for tool in &tools { self.active.remove(tool); }
        tools
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_one_device_keeps_other_tools_active() {
        let mut state = ProximityState::<u8, u8>::default();
        state.enter(1, 10);
        state.enter(2, 20);
        assert_eq!(state.cancel_device(&10), vec![1]);
        assert!(!state.contains(&1));
        assert!(state.contains(&2));
    }

    #[test]
    fn lock_transition_cancels_active_tools_and_requires_new_proximity() {
        let mut state = ProximityState::<u8>::default();
        state.enter(1, ());
        assert!(state.contains(&1));
        assert_eq!(state.cancel_all(), vec![1]);
        assert!(!state.contains(&1));
        assert!(state.cancel_all().is_empty());
    }
    #[test]
    fn tablet_input_wakes_powered_off_outputs() {
        let source = include_str!("../drivers/tty/mod.rs");
        let wake = source.split("let wake_before = match &event").nth(1).unwrap().split("match &event {").next().unwrap();
        for variant in ["TabletToolProximity", "TabletToolAxis", "TabletToolTip", "TabletToolButton"] {
            assert!(wake.contains(variant), "{variant} must trigger DPMS wake");
        }
    }

    #[test]
    fn cluster_composer_installs_tablet_isolation() {
        let actions = include_str!("actions.rs");
        let cluster = actions.split("SessionControl::ClusterMode =>").nth(1).unwrap().split("SessionControl::").next().unwrap();
        assert!(cluster.contains("tablet::cancel_all(session)"), "composer entry must cancel active strokes");
        assert!(include_str!("tablet.rs").contains("session.shell.cluster_composer.is_active()"), "composer must suppress tablet input");
    }

    #[test]
    fn capture_and_confirmation_install_tablet_isolation() {
        let capture = include_str!("../../capture/mod.rs");
        let state = include_str!("../state.rs");
        let input = include_str!("../input.rs");
        let begin_capture = capture.split("fn begin_modal_capture<").nth(1).unwrap().split("fn finish_modal_capture<").next().unwrap();
        assert!(begin_capture.contains("tablet::cancel_all(session)"), "capture entry must cancel existing tablet strokes");
        for name in ["request_cluster_dissolution", "show_exit_confirmation"] {
            let method = state.split(&format!("fn {name}(")).nth(1).unwrap().split("\n    }").next().unwrap();
            assert!(method.contains("tablet::cancel_all(self)"), "{name} must cancel existing tablet strokes");
        }
        let tablet = input.find("super::tablet::handle(session, event)").unwrap();
        let modal = input.find("modal::intercept_early(session, event)").unwrap();
        assert!(tablet < modal, "tablet isolation must run before modal interception");
        assert!(include_str!("tablet.rs").contains("session.capture.is_active()"), "capture must suppress tablet input");
        assert!(include_str!("tablet.rs").contains("confirmation_modal_active()"), "confirmations must suppress tablet input");
    }

}
