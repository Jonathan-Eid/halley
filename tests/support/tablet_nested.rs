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
    motions: Vec<(f64, f64)>,
    frames: usize,
    proximity_outs: usize,
    events: Vec<&'static str>,
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
            tool::Event::Pressure { pressure } => {
                state.tablet.pressures.push(pressure);
                state.tablet.events.push("pressure");
            }
            tool::Event::Motion { x, y } => {
                state.tablet.events.push("motion");
                state.tablet.motions.push((x, y));
            }
            tool::Event::Down { .. } => state.tablet.events.push("down"),
            tool::Event::Up => state.tablet.events.push("up"),
            tool::Event::Frame { .. } => state.tablet.frames += 1,
            _ => {}
        }
    }
}

#[test]
#[ignore = "requires a Wayland desktop, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn tablet_pressure_and_surface_focus_are_delivered() {
    let mut fixture = Fixture::with_two_outputs(
        "animations:\n enabled false\nend\ninput:\n tablet:\n  map-to-output \"winit\"\n end\n devices:\n  \"Test tablet 1\":\n   map-to-output \"winit-2\"\n  end\n end\nend\n",
    );
    let input = fullscreen_multi_output::Input::new(&fixture);
    input.command("rotate-second-180");
    let (mut queue, mut client) = native_window_named(
        &fixture,
        "Tablet target",
        "halley.test.tablet",
        [0, u32::MAX, 0, u32::MAX],
    );
    let node = fixture.node("Tablet target");
    fullscreen_multi_output::place(&mut fixture, node.id, true);
    let _keyboard = observe_keyboard(&mut queue, &mut client);
    let (_other_queue, _other) = native_window_named(
        &fixture,
        "Other window",
        "halley.test.other",
        [0, u32::MAX, 0, u32::MAX],
    );
    let other_node = fixture.node("Other window");
    fullscreen_multi_output::place(&mut fixture, other_node.id, false);
    queue.roundtrip(&mut client).unwrap();
    assert!(
        !client.keyboard_focused,
        "other window must have focus before pen down"
    );
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
    // Center of the presented content, converted to normalized configured-output coordinates.
    let state = input.state();
    let window = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == node.id)
        .unwrap();
    let rect = window["client_geometry"].as_array().unwrap();
    let x = rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() * 0.3;
    let y = rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() * 0.4;
    let output = state["outputs_geometry"]["winit-2"].as_array().unwrap();
    let nx = 1.0 - (x - output[0].as_f64().unwrap()) / output[2].as_f64().unwrap();
    let ny = 1.0 - (y - output[1].as_f64().unwrap()) / output[3].as_f64().unwrap();
    // Enter on the empty output corner, then move onto the client with pressure.
    // The axis must be delivered to the newly focused surface, not the old focus.
    input.command("tablet-proximity 1 42 1.0 1.0 0.0 in");
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
    let (mx, my) = client.tablet.motions.last().copied().unwrap();
    assert!(
        (mx - rect[2].as_f64().unwrap() * 0.3).abs() < 2.0
            && (my - rect[3].as_f64().unwrap() * 0.4).abs() < 2.0,
        "transformed surface position: {mx}, {my}"
    );

    client.tablet.events.clear();
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.75 down"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.tablet.events, vec!["motion", "pressure", "down"]);
    assert!(
        client.keyboard_focused,
        "tip down should focus the target window"
    );
    assert!((49151..=49152).contains(client.tablet.pressures.last().unwrap()));
    client.tablet.events.clear();
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.0 up"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(&client.tablet.events[..2], &["motion", "pressure"]);
    assert!(client.tablet.events[2..].contains(&"up"));

    // A coordinate on the exclusive output boundary has no route. Pressure
    // must not be sent to the previously focused client in that case.
    let pressure_count = client.tablet.pressures.len();
    input.command("tablet-axis 1 42 0.0 0.0 0.8 in");
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.tablet.pressures.len(), pressure_count);
    assert_eq!(
        client.tablet.proximity_outs, 1,
        "route loss must clear client focus"
    );
}

// Exercise real protocol events, not the spelling or ordering of source code.
fn isolation_scenario(lock: bool) {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = fullscreen_multi_output::Input::new(&fixture);
    let (mut queue, mut client) = native_window_named(
        &fixture,
        "Isolation target",
        "halley.test.isolation",
        [0, u32::MAX, 0, u32::MAX],
    );
    let node = fixture.node("Isolation target");
    fullscreen_multi_output::place(&mut fixture, node.id, false);
    let handle = queue.handle();
    let registry = client.registry.as_ref().unwrap();
    let wl_seat: wl_seat::WlSeat = registry.bind(client.globals["wl_seat"], 1, &handle, ());
    let manager: manager::ZwpTabletManagerV2 =
        registry.bind(client.globals["zwp_tablet_manager_v2"], 1, &handle, ());
    let _seat = manager.get_tablet_seat(&wl_seat, &handle, ());
    queue.roundtrip(&mut client).unwrap();
    input.command("tablet-add 1");
    let state = input.state();
    let window = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == node.id)
        .unwrap();
    let rect = window["rect"].as_array().unwrap();
    let output = state["primary_geometry"].as_array().unwrap();
    let nx = (rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() / 2.0
        - output[0].as_f64().unwrap())
        / output[2].as_f64().unwrap();
    let ny = (rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() / 2.0
        - output[1].as_f64().unwrap())
        / output[3].as_f64().unwrap();
    input.command(&format!("tablet-proximity 1 42 {nx} {ny} 0.5 in"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.tablet.proximity,
        vec![client.surface.clone().unwrap()]
    );

    let mut request = None;
    if lock {
        use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1;
        let manager: ext_session_lock_manager_v1::ExtSessionLockManagerV1 =
            client.registry.as_ref().unwrap().bind(
                client.globals["ext_session_lock_manager_v1"],
                1,
                &handle,
                (),
            );
        let _lock = manager.lock(&handle, ());
        queue.roundtrip(&mut client).unwrap();
    } else {
        let path = fixture.path.join("halley/halley.sock");
        request = Some(thread::spawn(move || {
            halley_ipc::Connection::connect_to(&path)
                .unwrap()
                .request(
                    &Request::ChooseSource(halley_ipc::SourceChooserRequest {
                        request_handle: "tablet-isolation".into(),
                        source_types: halley_ipc::SOURCE_MONITOR,
                    }),
                    &[],
                )
                .unwrap()
                .response
        }));
        wait_for("capture active", || {
            input.state()["capture_active"]
                .as_bool()
                .unwrap()
                .then_some(())
        });
        queue.roundtrip(&mut client).unwrap();
    }
    assert_eq!(
        client.tablet.proximity_outs, 1,
        "modal entry must cancel the active tool"
    );
    let before = (
        client.tablet.proximity.len(),
        client.tablet.pressures.len(),
        client.tablet.frames,
    );
    input.command(&format!("tablet-proximity 1 42 {nx} {ny} 0.7 in"));
    input.command(&format!("tablet-axis 1 42 {nx} {ny} 0.8 in"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        (
            client.tablet.proximity.len(),
            client.tablet.pressures.len(),
            client.tablet.frames
        ),
        before,
        "modal input leaked to ordinary client"
    );
    if let Some(request) = request {
        input.key(1, true);
        input.key(1, false);
        request.join().unwrap();
        input.command(&format!("tablet-axis 1 42 {nx} {ny} 0.9 in"));
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(
            (
                client.tablet.proximity.len(),
                client.tablet.pressures.len(),
                client.tablet.frames
            ),
            before,
            "stale tool must not resume after capture"
        );
        input.command(&format!("tablet-proximity 1 42 {nx} {ny} 0.5 in"));
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(
            client.tablet.proximity.len(),
            before.0 + 1,
            "fresh proximity must resume delivery"
        );
    }
}

#[test]
#[ignore = "requires a Wayland desktop, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn capture_cancels_and_isolates_tablet_protocol_stream() {
    isolation_scenario(false);
}

#[test]
#[ignore = "requires a Wayland desktop, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn lock_cancels_and_isolates_tablet_protocol_stream() {
    isolation_scenario(true);
}

#[test]
#[ignore = "requires a Wayland desktop, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn tablet_recovers_after_proximity_without_output_geometry() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = fullscreen_multi_output::Input::new(&fixture);
    let (mut queue, mut client) = native_window_named(
        &fixture,
        "Geometry target",
        "halley.test.geometry",
        [0, u32::MAX, 0, u32::MAX],
    );
    let node = fixture.node("Geometry target");
    fullscreen_multi_output::place(&mut fixture, node.id, false);
    let handle = queue.handle();
    let registry = client.registry.as_ref().unwrap();
    let wl_seat: wl_seat::WlSeat = registry.bind(client.globals["wl_seat"], 1, &handle, ());
    let manager: manager::ZwpTabletManagerV2 =
        registry.bind(client.globals["zwp_tablet_manager_v2"], 1, &handle, ());
    let _seat = manager.get_tablet_seat(&wl_seat, &handle, ());
    queue.roundtrip(&mut client).unwrap();
    input.command("tablet-add 1");
    let state = input.state();
    let window = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == node.id)
        .unwrap();
    let rect = window["client_geometry"].as_array().unwrap();
    let output = state["primary_geometry"].as_array().unwrap();
    let nx = (rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() / 2.0
        - output[0].as_f64().unwrap())
        / output[2].as_f64().unwrap();
    let ny = (rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() / 2.0
        - output[1].as_f64().unwrap())
        / output[3].as_f64().unwrap();
    input.command(&format!(
        "tablet-proximity-no-geometry 1 42 {nx} {ny} 0.5 in"
    ));
    input.command(&format!("tablet-axis 1 42 {nx} {ny} 0.6 in"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.tablet.proximity,
        vec![client.surface.clone().unwrap()],
        "motion must recover without a second proximity-in"
    );
    assert!(!client.tablet.pressures.is_empty());
    input.command(&format!("tablet-axis-no-geometry 1 42 {nx} {ny} 0.7 in"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.tablet.proximity_outs, 1,
        "geometry loss clears stale surface focus"
    );
    input.command(&format!("tablet-axis 1 42 {nx} {ny} 0.8 in"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.tablet.proximity.len(),
        2,
        "focus returns after geometry recovery"
    );
    client.tablet.events.clear();
    input.command(&format!("tablet-tip-no-geometry 1 42 {nx} {ny} 0.8 down"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.tablet.proximity_outs, 2,
        "tip without geometry must clear focus"
    );
    assert!(
        !client.tablet.events.contains(&"down"),
        "tip must not reach stale focus"
    );
}
