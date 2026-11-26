//! Real tablet-v2 client driven through the test-only backend input seam.
use super::*;
use wayland_protocols::wp::tablet::zv2::client::{
    zwp_tablet_manager_v2 as manager, zwp_tablet_seat_v2 as seat, zwp_tablet_tool_v2 as tool,
    zwp_tablet_v2 as tablet,
};

#[derive(Default)]
pub(super) struct Observations {
    tablets: usize,
    tools: usize,
    proximity: Vec<wl_surface::WlSurface>,
    pressures: Vec<u32>,
    frames: usize,
    proximity_outs: usize,
}
delegate_noop!(NativeState: ignore manager::ZwpTabletManagerV2);
impl Dispatch<seat::ZwpTabletSeatV2, ()> for NativeState {
    wayland_client::event_created_child!(NativeState, seat::ZwpTabletSeatV2, [
        seat::EVT_TABLET_ADDED_OPCODE => (tablet::ZwpTabletV2, ()),
        seat::EVT_TOOL_ADDED_OPCODE => (tool::ZwpTabletToolV2, ())
    ]);
    fn event(
        state: &mut Self,
        _: &seat::ZwpTabletSeatV2,
        event: seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            seat::Event::TabletAdded { .. } => state.tablet.tablets += 1,
            seat::Event::ToolAdded { .. } => state.tablet.tools += 1,
            _ => {}
        }
    }
}
delegate_noop!(NativeState: ignore tablet::ZwpTabletV2);
impl Dispatch<tool::ZwpTabletToolV2, ()> for NativeState {
    fn event(
        state: &mut Self,
        _: &tool::ZwpTabletToolV2,
        event: tool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            tool::Event::ProximityIn { surface, .. } => state.tablet.proximity.push(surface),
            tool::Event::ProximityOut => state.tablet.proximity_outs += 1,
            tool::Event::Pressure { pressure } => state.tablet.pressures.push(pressure),
            tool::Event::Frame { .. } => state.tablet.frames += 1,
            _ => {}
        }
    }
}

#[test]
#[ignore = "requires a Wayland desktop, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn tablet_pressure_and_surface_focus_are_delivered() {
    let mut fixture = Fixture::with_two_outputs(
        "animations:\n enabled false\nend\ninput:\n touchscreen:\n  map-to-output \"winit-2\"\n end\nend\n",
    );
    let input = fullscreen_multi_output::Input::new(&fixture);
    let (mut queue, mut client) = native_window_named(
        &fixture,
        "Tablet target",
        "halley.test.tablet",
        [0, u32::MAX, 0, u32::MAX],
    );
    let node = fixture.node("Tablet target");
    fullscreen_multi_output::place(&mut fixture, node.id, false);
    let handle = queue.handle();
    let registry = client.registry.as_ref().unwrap();
    let wl_seat: wl_seat::WlSeat = registry.bind(client.globals["wl_seat"], 1, &handle, ());
    let manager: manager::ZwpTabletManagerV2 =
        registry.bind(client.globals["zwp_tablet_manager_v2"], 1, &handle, ());
    let _seat = manager.get_tablet_seat(&wl_seat, &handle, ());
    queue.roundtrip(&mut client).unwrap();
    input.command("tablet-add 1");
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.tablet.tablets, 1,
        "device addition must announce a tablet"
    );
    // Center of the presented content, converted to normalized primary-output coordinates.
    let state = input.state();
    let window = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == node.id)
        .unwrap();
    let rect = window["rect"].as_array().unwrap();
    let x = rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() / 2.0;
    let y = rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() / 2.0;
    let output = state["primary_geometry"].as_array().unwrap();
    let nx = (x - output[0].as_f64().unwrap()) / output[2].as_f64().unwrap();
    let ny = (y - output[1].as_f64().unwrap()) / output[3].as_f64().unwrap();
    // Enter on the empty output corner, then move onto the client with pressure.
    // The axis must be delivered to the newly focused surface, not the old focus.
    input.command("tablet-proximity 1 42 0.0 0.0 0.0 in");
    input.command(&format!("tablet-axis 1 42 {nx} {ny} 0.5 in"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.tablet.tools, 1);
    assert_eq!(
        client.tablet.proximity,
        vec![client.surface.clone().unwrap()]
    );
    assert!(
        client
            .tablet
            .pressures
            .iter()
            .any(|p| (32767..=32768).contains(p)),
        "pressure: {:?}",
        client.tablet.pressures
    );
    assert!(client.tablet.frames > 0);

    // A coordinate on the exclusive output boundary has no route. Pressure
    // must not be sent to the previously focused client in that case.
    let pressure_count = client.tablet.pressures.len();
    input.command("tablet-axis 1 42 1.0 1.0 0.8 in");
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.tablet.pressures.len(), pressure_count);
    assert_eq!(
        client.tablet.proximity_outs, 1,
        "route loss must clear client focus"
    );
}
