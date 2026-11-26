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
    tool: Option<tool::ZwpTabletToolV2>,
    serial: Option<u32>,
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
            seat::Event::ToolAdded { id } => {
                state.tablet.tools += 1;
                state.tablet.tool = Some(id);
            }
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
            tool::Event::ProximityIn {
                surface, serial, ..
            } => {
                state.tablet.proximity.push(surface);
                state.tablet.serial = Some(serial);
            }
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
    let registry = client.registry.as_ref().unwrap().clone();
    let wl_seat: wl_seat::WlSeat = registry.bind(client.globals["wl_seat"], 1, &handle, ());
    let manager: manager::ZwpTabletManagerV2 =
        registry.bind(client.globals["zwp_tablet_manager_v2"], 1, &handle, ());
    let _seat = manager.get_tablet_seat(&wl_seat, &handle, ());
    let _pointer = wl_seat.get_pointer(&handle, ());
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
    let pointer = input.state()["pointer"].as_array().unwrap().clone();
    assert!((pointer[0].as_f64().unwrap() - x).abs() < 2.0);
    assert!((pointer[1].as_f64().unwrap() - y).abs() < 2.0);
    let tool = client.tablet.tool.clone().unwrap();
    let serial = client.tablet.serial.unwrap();
    let compositor: wl_compositor::WlCompositor =
        registry.bind(client.globals["wl_compositor"], 1, &handle, ());
    let cursor = compositor.create_surface(&handle, ());
    tool.set_cursor(serial.wrapping_add(1), Some(&cursor), 0, 0);
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        input.state()["client_cursor_surface"],
        false,
        "invalid serial must not set cursor"
    );
    tool.set_cursor(serial, Some(&cursor), 0, 0);
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        input.state()["client_cursor_surface"],
        true,
        "focused tablet client can set cursor"
    );
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
    assert!(
        client.pointer_buttons.is_empty(),
        "tablet-aware client must not receive duplicate pointer clicks"
    );

    // A coordinate on the exclusive output boundary has no route. Pressure
    // must not be sent to the previously focused client in that case.
    let pressure_count = client.tablet.pressures.len();
    input.command("tablet-axis 1 42 0.0 0.0 0.8 in");
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        input.state()["client_cursor_surface"],
        false,
        "focus loss clears cursor"
    );
    tool.set_cursor(serial, Some(&cursor), 0, 0);
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        input.state()["client_cursor_surface"],
        false,
        "unfocused client cannot restore cursor"
    );
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

    // Enter the modal while the pen is touching the surface, so cancellation
    // also exercises an active tip rather than hover alone.
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.75 down"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.tablet.events.last(), Some(&"down"));

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
        // This checks isolation during lock acquisition, not a confirmed lock
        // or unlock: the two-output fixture does not confirm this request.
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
    let event_count = client.tablet.events.len();
    input.command(&format!("tablet-proximity 1 42 {nx} {ny} 0.7 in"));
    input.command(&format!("tablet-axis 1 42 {nx} {ny} 0.8 in"));
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.8 down"));
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.0 up"));
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
    assert_eq!(
        client.tablet.events.len(),
        event_count,
        "modal tip leaked to ordinary client"
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

#[test]
#[ignore = "requires a Wayland desktop, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn pointer_only_client_receives_pen_tip() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = fullscreen_multi_output::Input::new(&fixture);
    let (mut queue, mut client) = native_window_named(
        &fixture,
        "Pointer fallback",
        "halley.test.fallback",
        [0, u32::MAX, 0, u32::MAX],
    );
    let node = fixture.node("Pointer fallback");
    fullscreen_multi_output::place(&mut fixture, node.id, false);
    let handle = queue.handle();
    let seat: wl_seat::WlSeat =
        client
            .registry
            .as_ref()
            .unwrap()
            .bind(client.globals["wl_seat"], 8, &handle, ());
    let _pointer = seat.get_pointer(&handle, ());
    queue.roundtrip(&mut client).unwrap();
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
    input.command("tablet-add 1");
    input.command(&format!("tablet-proximity 1 42 {nx} {ny} 0.0 in"));
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.8 down"));
    input.command("tablet-axis 1 42 1.0 1.0 0.8 in");
    input.command("tablet-tip 1 42 1.0 1.0 0.0 up");
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.pointer_buttons,
        vec![
            (
                272,
                wayland_client::WEnum::Value(
                    wayland_client::protocol::wl_pointer::ButtonState::Pressed
                )
            ),
            (
                272,
                wayland_client::WEnum::Value(
                    wayland_client::protocol::wl_pointer::ButtonState::Released
                )
            )
        ]
    );
    // Hovering off a pointer-only surface must end its wl_pointer focus.
    input.command(&format!("tablet-axis 1 42 {nx} {ny} 0.0 in"));
    queue.roundtrip(&mut client).unwrap();
    assert!(
        client.pointer_warp.enter.is_some(),
        "pen must focus the client"
    );
    input.command("tablet-axis 1 42 1.0 1.0 0.0 in");
    queue.roundtrip(&mut client).unwrap();
    assert!(
        client.pointer_warp.enter.is_none(),
        "background must clear stale pointer focus"
    );
    // Entering capture cancels a held emulated button and suppresses later tip input.
    client.pointer_buttons.clear();
    input.command(&format!("tablet-axis 1 42 {nx} {ny} 0.0 in"));
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.8 down"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.pointer_buttons.len(), 1);
    let path = fixture.path.join("halley/halley.sock");
    let request = thread::spawn(move || {
        halley_ipc::Connection::connect_to(&path)
            .unwrap()
            .request(
                &Request::ChooseSource(halley_ipc::SourceChooserRequest {
                    request_handle: "tablet-fallback-isolation".into(),
                    source_types: halley_ipc::SOURCE_MONITOR,
                }),
                &[],
            )
            .unwrap()
            .response
    });
    wait_for("capture active", || {
        input.state()["capture_active"]
            .as_bool()
            .unwrap()
            .then_some(())
    });
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.pointer_buttons.len(),
        2,
        "capture must release held fallback button"
    );
    input.command(&format!("tablet-proximity 1 42 {nx} {ny} 0.0 in"));
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.8 down"));
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.0 up"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.pointer_buttons.len(),
        2,
        "capture must suppress fallback input"
    );
    input.key(1, true);
    input.key(1, false);
    request.join().unwrap();
}

#[test]
#[ignore = "requires a Wayland desktop, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn lock_releases_held_pointer_fallback() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = fullscreen_multi_output::Input::new(&fixture);
    let (mut queue, mut client) = native_window_named(
        &fixture,
        "Pointer fallback",
        "halley.test.fallback",
        [0, u32::MAX, 0, u32::MAX],
    );
    let node = fixture.node("Pointer fallback");
    fullscreen_multi_output::place(&mut fixture, node.id, false);
    let handle = queue.handle();
    let seat: wl_seat::WlSeat =
        client
            .registry
            .as_ref()
            .unwrap()
            .bind(client.globals["wl_seat"], 8, &handle, ());
    let _pointer = seat.get_pointer(&handle, ());
    queue.roundtrip(&mut client).unwrap();
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
    input.command("tablet-add 1");
    input.command(&format!("tablet-proximity 1 42 {nx} {ny} 0.0 in"));
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.8 down"));
    input.command("tablet-axis 1 42 1.0 1.0 0.8 in");
    input.command("tablet-tip 1 42 1.0 1.0 0.0 up");
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.pointer_buttons,
        vec![
            (
                272,
                wayland_client::WEnum::Value(
                    wayland_client::protocol::wl_pointer::ButtonState::Pressed
                )
            ),
            (
                272,
                wayland_client::WEnum::Value(
                    wayland_client::protocol::wl_pointer::ButtonState::Released
                )
            )
        ]
    );
    client.pointer_buttons.clear();
    input.command(&format!("tablet-axis 1 42 {nx} {ny} 0.0 in"));
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.8 down"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.pointer_buttons.len(), 1);
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
    assert_eq!(
        client.pointer_buttons.len(),
        2,
        "lock must release held fallback button"
    );
    assert_eq!(
        client.pointer_buttons[1].1,
        wayland_client::WEnum::Value(wayland_client::protocol::wl_pointer::ButtonState::Released)
    );
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.0 up"));
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.pointer_buttons.len(),
        2,
        "lock must suppress further fallback input"
    );
}

#[test]
#[ignore = "requires a Wayland desktop, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn pointer_lock_blocks_tablet_fallback_motion() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = fullscreen_multi_output::Input::new(&fixture);
    let (mut queue, mut client) = native_window_named(
        &fixture,
        "Pointer fallback",
        "halley.test.fallback",
        [0, u32::MAX, 0, u32::MAX],
    );
    let node = fixture.node("Pointer fallback");
    fullscreen_multi_output::place(&mut fixture, node.id, false);
    let handle = queue.handle();
    let seat: wl_seat::WlSeat =
        client
            .registry
            .as_ref()
            .unwrap()
            .bind(client.globals["wl_seat"], 8, &handle, ());
    let _pointer = seat.get_pointer(&handle, ());
    queue.roundtrip(&mut client).unwrap();
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
    input.command("tablet-add 1");
    input.command(&format!("tablet-proximity 1 42 {nx} {ny} 0.0 in"));
    queue.roundtrip(&mut client).unwrap();
    use wayland_protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1 as constraints;
    let manager: constraints::ZwpPointerConstraintsV1 = client.registry.as_ref().unwrap().bind(
        client.globals["zwp_pointer_constraints_v1"],
        1,
        &handle,
        (),
    );
    use wayland_protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_manager_v1 as relative_manager;
    let relative_manager: relative_manager::ZwpRelativePointerManagerV1 =
        client.registry.as_ref().unwrap().bind(
            client.globals["zwp_relative_pointer_manager_v1"],
            1,
            &handle,
            (),
        );
    let _relative = relative_manager.get_relative_pointer(&_pointer, &handle, ());
    let _lock = manager.lock_pointer(
        client.surface.as_ref().unwrap(),
        &_pointer,
        None,
        constraints::Lifetime::Persistent,
        &handle,
        (),
    );
    wait_for("pointer lock", || {
        queue.roundtrip(&mut client).unwrap();
        client.pointer_warp.locked.then_some(())
    });
    let motions = client.pointer_warp.motions;
    input.command(&format!(
        "tablet-axis 1 42 {} {} 0.5 in",
        nx + 0.03,
        ny + 0.03
    ));
    queue.roundtrip(&mut client).unwrap();
    input.command(&format!(
        "tablet-axis 1 42 {} {} 0.5 in",
        nx + 0.06,
        ny + 0.06
    ));
    queue.roundtrip(&mut client).unwrap();
    let deltas = &client.pointer_warp.relative_deltas;
    assert_eq!(deltas.len(), 2, "expected two relative motions: {deltas:?}");
    assert!(
        (deltas[0].0 - deltas[1].0).abs() < 0.1,
        "equal physical movements must produce equal relative deltas: {deltas:?}"
    );
    assert_eq!(
        client.pointer_warp.motions, motions,
        "locked pointer must not receive absolute tablet motion"
    );
}

#[test]
#[ignore = "requires a Wayland desktop, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn pointer_confinement_clamps_tablet_fallback_motion() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = fullscreen_multi_output::Input::new(&fixture);
    let (mut queue, mut client) = native_window_named(
        &fixture,
        "Pointer fallback",
        "halley.test.fallback",
        [0, u32::MAX, 0, u32::MAX],
    );
    let node = fixture.node("Pointer fallback");
    fullscreen_multi_output::place(&mut fixture, node.id, false);
    let handle = queue.handle();
    let seat: wl_seat::WlSeat =
        client
            .registry
            .as_ref()
            .unwrap()
            .bind(client.globals["wl_seat"], 8, &handle, ());
    let _pointer = seat.get_pointer(&handle, ());
    queue.roundtrip(&mut client).unwrap();
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
    input.command("tablet-add 1");
    input.command(&format!("tablet-proximity 1 42 {nx} {ny} 0.0 in"));
    queue.roundtrip(&mut client).unwrap();
    use wayland_protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1 as constraints;
    let manager: constraints::ZwpPointerConstraintsV1 = client.registry.as_ref().unwrap().bind(
        client.globals["zwp_pointer_constraints_v1"],
        1,
        &handle,
        (),
    );
    let _confine = manager.confine_pointer(
        client.surface.as_ref().unwrap(),
        &_pointer,
        None,
        constraints::Lifetime::Persistent,
        &handle,
        (),
    );
    wait_for("pointer confinement", || {
        queue.roundtrip(&mut client).unwrap();
        client.pointer_warp.confined.then_some(())
    });
    let before = client.pointer_warp.position.unwrap();
    input.command(&format!("tablet-axis 1 42 {} {} 0.5 in", nx + 0.01, ny));
    queue.roundtrip(&mut client).unwrap();
    input.command(&format!("tablet-axis 1 42 {} {} 0.5 in", nx + 0.02, ny));
    queue.roundtrip(&mut client).unwrap();
    assert!(client.pointer_warp.confined);
    assert!(
        (client.pointer_warp.position.unwrap().0 - before.0 - 0.02 * output[2].as_f64().unwrap())
            .abs()
            < 2.0
    );
    input.command("tablet-axis 1 42 1.0 1.0 0.5 in");
    queue.roundtrip(&mut client).unwrap();
    assert!(
        client.pointer_warp.confined,
        "tablet motion must not drop confinement"
    );
    assert!(
        client.pointer_warp.enter.is_some(),
        "tablet motion must preserve pointer focus"
    );
    let (x, y) = client.pointer_warp.position.unwrap();
    assert!(
        x > rect[2].as_f64().unwrap() - 2.0,
        "out-of-region motion must reach the confinement edge: {x}, {y}"
    );
    assert!(
        x >= 0.0 && x <= rect[2].as_f64().unwrap() && y >= 0.0 && y <= rect[3].as_f64().unwrap(),
        "confined position must remain within the surface: {x}, {y}"
    );
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn tablet_titlebar_drag_moves_window() {
    let mut fixture = Fixture::with_two_outputs(
        "animations:\n enabled false\nend\ndecorations:\n titlebars:\n  enabled true\n  height 32\n end\nend\n",
    );
    let input = fullscreen_multi_output::Input::new(&fixture);
    let (mut queue, mut client) = native_window_named(
        &fixture,
        "Pen drag",
        "halley.test.pen-drag",
        [0, u32::MAX, 0, u32::MAX],
    );
    let manager: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
        client.registry.as_ref().unwrap().bind(
            client.globals["zxdg_decoration_manager_v1"],
            1,
            &queue.handle(),
            (),
        );
    let decoration =
        manager.get_toplevel_decoration(client.toplevel.as_ref().unwrap(), &queue.handle(), ());
    decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
    queue.roundtrip(&mut client).unwrap();
    queue.roundtrip(&mut client).unwrap();
    let node = fixture.node("Pen drag");
    fullscreen_multi_output::place(&mut fixture, node.id, false);
    let state = input.state();
    let window = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == node.id)
        .unwrap();
    let r = window["rect"].as_array().unwrap();
    let client = window["client_geometry"].as_array().unwrap();
    let output = state["primary_geometry"].as_array().unwrap();
    let x = r[0].as_f64().unwrap() + r[2].as_f64().unwrap() / 2.0;
    let scale = r[3].as_f64().unwrap() / client[3].as_f64().unwrap();
    let y = r[1].as_f64().unwrap() - 16.0 * scale;
    let nx = |x: f64| (x - output[0].as_f64().unwrap()) / output[2].as_f64().unwrap();
    // Winit exposes its primary output with a vertically flipped transform.
    let ny = |y: f64| 1.0 - (y - output[1].as_f64().unwrap()) / output[3].as_f64().unwrap();
    input.command("tablet-add 1");
    input.command(&format!("tablet-proximity 1 42 {} {} 0.0 in", nx(x), ny(y)));
    input.command(&format!("tablet-tip 1 42 {} {} 0.8 down", nx(x), ny(y)));
    input.command(&format!(
        "tablet-axis 1 42 {} {} 0.8 in",
        nx(x + 60.0),
        ny(y + 30.0)
    ));
    assert_eq!(
        input.state()["moving"],
        true,
        "pen drag must start compositor move"
    );
    let moved = input.state()["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == node.id)
        .unwrap()["client_geometry"]
        .clone();
    assert_ne!(
        moved, window["client_geometry"],
        "pen motion must move the window"
    );
    input.command(&format!(
        "tablet-tip 1 42 {} {} 0.0 up",
        nx(x + 60.0),
        ny(y + 30.0)
    ));
    assert_eq!(
        input.state()["moving"],
        false,
        "pen release must finish move"
    );
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn locked_pointer_ignores_physical_titlebar_drag() {
    let mut fixture = Fixture::with_two_outputs(
        "animations:\n enabled false\nend\ndecorations:\n titlebars:\n  enabled true\n  height 32\n end\nend\n",
    );
    let input = fullscreen_multi_output::Input::new(&fixture);
    let (mut queue, mut client) = native_window_named(
        &fixture,
        "Pen drag",
        "halley.test.pen-drag",
        [0, u32::MAX, 0, u32::MAX],
    );
    let manager: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
        client.registry.as_ref().unwrap().bind(
            client.globals["zxdg_decoration_manager_v1"],
            1,
            &queue.handle(),
            (),
        );
    let decoration =
        manager.get_toplevel_decoration(client.toplevel.as_ref().unwrap(), &queue.handle(), ());
    decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
    queue.roundtrip(&mut client).unwrap();
    queue.roundtrip(&mut client).unwrap();
    let node = fixture.node("Pen drag");
    fullscreen_multi_output::place(&mut fixture, node.id, false);
    let state = input.state();
    let window = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == node.id)
        .unwrap();
    let r = window["rect"].as_array().unwrap();
    let client_rect = window["client_geometry"].as_array().unwrap();
    let output = state["primary_geometry"].as_array().unwrap();
    let x = r[0].as_f64().unwrap() + r[2].as_f64().unwrap() / 2.0;
    let scale = r[3].as_f64().unwrap() / client_rect[3].as_f64().unwrap();
    let y = r[1].as_f64().unwrap() - 16.0 * scale;
    let nx = |x: f64| (x - output[0].as_f64().unwrap()) / output[2].as_f64().unwrap();
    // Winit exposes its primary output with a vertically flipped transform.
    let ny = |y: f64| 1.0 - (y - output[1].as_f64().unwrap()) / output[3].as_f64().unwrap();
    let handle = queue.handle();
    let seat: wl_seat::WlSeat =
        client
            .registry
            .as_ref()
            .unwrap()
            .bind(client.globals["wl_seat"], 8, &handle, ());
    let pointer = seat.get_pointer(&handle, ());
    queue.roundtrip(&mut client).unwrap();
    input.command("tablet-add 1");
    let cy = client_rect[1].as_f64().unwrap() + client_rect[3].as_f64().unwrap() / 2.0;
    input.command(&format!(
        "tablet-proximity 1 42 {} {} 0.0 in",
        nx(x),
        ny(cy)
    ));
    queue.roundtrip(&mut client).unwrap();
    use wayland_protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1 as constraints;
    let manager: constraints::ZwpPointerConstraintsV1 = client.registry.as_ref().unwrap().bind(
        client.globals["zwp_pointer_constraints_v1"],
        1,
        &handle,
        (),
    );
    let _lock = manager.lock_pointer(
        client.surface.as_ref().unwrap(),
        &pointer,
        None,
        constraints::Lifetime::Persistent,
        &handle,
        (),
    );
    wait_for("pointer lock", || {
        queue.roundtrip(&mut client).unwrap();
        client.pointer_warp.locked.then_some(())
    });
    input.command(&format!("tablet-tip 1 42 {} {} 0.8 down", nx(x), ny(y)));
    input.command(&format!(
        "tablet-axis 1 42 {} {} 0.8 in",
        nx(x + 60.0),
        ny(y + 30.0)
    ));
    assert_eq!(
        input.state()["moving"],
        false,
        "locked pen must not start compositor move"
    );
    let moved = input.state()["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == node.id)
        .unwrap()["client_geometry"]
        .clone();
    assert_eq!(
        moved, window["client_geometry"],
        "locked pen must not move the window"
    );
    input.command(&format!(
        "tablet-tip 1 42 {} {} 0.0 up",
        nx(x + 60.0),
        ny(y + 30.0)
    ));
    assert_eq!(
        input.state()["moving"],
        false,
        "pen release must finish move"
    );
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn confined_pointer_ignores_physical_titlebar_drag() {
    let mut fixture = Fixture::with_two_outputs(
        "animations:\n enabled false\nend\ndecorations:\n titlebars:\n  enabled true\n  height 32\n end\nend\n",
    );
    let input = fullscreen_multi_output::Input::new(&fixture);
    let (mut queue, mut client) = native_window_named(
        &fixture,
        "Pen drag",
        "halley.test.pen-drag",
        [0, u32::MAX, 0, u32::MAX],
    );
    let manager: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
        client.registry.as_ref().unwrap().bind(
            client.globals["zxdg_decoration_manager_v1"],
            1,
            &queue.handle(),
            (),
        );
    let decoration =
        manager.get_toplevel_decoration(client.toplevel.as_ref().unwrap(), &queue.handle(), ());
    decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
    queue.roundtrip(&mut client).unwrap();
    queue.roundtrip(&mut client).unwrap();
    let node = fixture.node("Pen drag");
    fullscreen_multi_output::place(&mut fixture, node.id, false);
    let state = input.state();
    let window = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == node.id)
        .unwrap();
    let r = window["rect"].as_array().unwrap();
    let client_rect = window["client_geometry"].as_array().unwrap();
    let output = state["primary_geometry"].as_array().unwrap();
    let x = r[0].as_f64().unwrap() + r[2].as_f64().unwrap() / 2.0;
    let scale = r[3].as_f64().unwrap() / client_rect[3].as_f64().unwrap();
    let y = r[1].as_f64().unwrap() - 16.0 * scale;
    let nx = |x: f64| (x - output[0].as_f64().unwrap()) / output[2].as_f64().unwrap();
    // Winit exposes its primary output with a vertically flipped transform.
    let ny = |y: f64| 1.0 - (y - output[1].as_f64().unwrap()) / output[3].as_f64().unwrap();
    let handle = queue.handle();
    let seat: wl_seat::WlSeat =
        client
            .registry
            .as_ref()
            .unwrap()
            .bind(client.globals["wl_seat"], 8, &handle, ());
    let pointer = seat.get_pointer(&handle, ());
    queue.roundtrip(&mut client).unwrap();
    input.command("tablet-add 1");
    let cy = client_rect[1].as_f64().unwrap() + client_rect[3].as_f64().unwrap() / 2.0;
    input.command(&format!(
        "tablet-proximity 1 42 {} {} 0.0 in",
        nx(x),
        ny(cy)
    ));
    queue.roundtrip(&mut client).unwrap();
    use wayland_protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1 as constraints;
    let manager: constraints::ZwpPointerConstraintsV1 = client.registry.as_ref().unwrap().bind(
        client.globals["zwp_pointer_constraints_v1"],
        1,
        &handle,
        (),
    );
    let _lock = manager.confine_pointer(
        client.surface.as_ref().unwrap(),
        &pointer,
        None,
        constraints::Lifetime::Persistent,
        &handle,
        (),
    );
    wait_for("pointer lock", || {
        queue.roundtrip(&mut client).unwrap();
        client.pointer_warp.confined.then_some(())
    });
    input.command(&format!("tablet-tip 1 42 {} {} 0.8 down", nx(x), ny(y)));
    input.command(&format!(
        "tablet-axis 1 42 {} {} 0.8 in",
        nx(x + 60.0),
        ny(y + 30.0)
    ));
    assert_eq!(
        input.state()["moving"],
        false,
        "confined pen must not start compositor move"
    );
    let moved = input.state()["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == node.id)
        .unwrap()["client_geometry"]
        .clone();
    assert_eq!(
        moved, window["client_geometry"],
        "confined pen must not move the window"
    );
    input.command(&format!(
        "tablet-tip 1 42 {} {} 0.0 up",
        nx(x + 60.0),
        ny(y + 30.0)
    ));
    assert_eq!(
        input.state()["moving"],
        false,
        "pen release must finish move"
    );
}
