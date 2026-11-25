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
}
