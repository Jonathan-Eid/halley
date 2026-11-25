//! Two-output nested server compiled only into test executables.
//! Real protocol clients and the production input/session/rendering paths run
//! unchanged; the extra output is offscreen and input is isolated from the host.
use super::*;
use calloop::{Interest, Mode, PostAction, generic::Generic};
use smithay::backend::input::{AbsolutePositionEvent, Device, DeviceCapability};
use smithay::backend::input::{
    ButtonState, Event, InputBackend, InputEvent, InputTime, KeyState, KeyboardKeyEvent, Keycode,
    PointerButtonEvent, PointerMotionEvent, UnusedEvent,
};
use smithay::backend::input::{
    ProximityState, TabletToolAxisEvent, TabletToolCapabilities, TabletToolDescriptor,
    TabletToolEvent, TabletToolProximityEvent, TabletToolType,
};
use smithay::backend::winit::WinitVirtualDevice;
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use smithay::utils::Point;
use std::os::unix::net::UnixDatagram;

pub(super) fn output_state(output: &Output) -> super::super::output::OutputState {
    super::super::output::OutputState {
        output: output.clone(),
        enabled: true,
        mode: output.current_mode().unwrap(),
        location: output.current_location(),
        transform: output.current_transform(),
        scale: output.current_scale().fractional_scale(),
        adaptive_sync: false,
        adaptive_sync_supported: false,
    }
}

pub(super) fn output_info(output: &Output) -> halley_ipc::OutputInfo {
    halley_ipc::OutputInfo {
        name: output.name(),
        modes: vec![crate::ipc::mode_info(output.current_mode().unwrap(), true)],
        current_mode: Some(0),
        offset_x: output.current_location().x,
        offset_y: output.current_location().y,
        scale: output.current_scale().fractional_scale(),
        vrr: "off".into(),
        vrr_supported: false,
        vrr_active: false,
    }
}

pub(super) fn install(app: &mut App, handle: &LoopHandle<'static, App>) {
    let Some(path) = std::env::var_os("HALLEY_TEST_WINIT_CONTROL") else {
        return;
    };
    let output = Output::new(
        "winit-2".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Halley test".into(),
            model: "offscreen".into(),
            serial_number: "2".into(),
        },
    );
    let mode = OutputMode {
        size: (1024, 768).into(),
        refresh: 60_000,
    };
    let default_x = app.driver.backend.logical_size().w;
    let mut positions = std::env::var("HALLEY_TEST_OUTPUT_LAYOUT")
        .map(|value| {
            let coordinates = value
                .split(',')
                .map(|part| part.parse::<i32>().expect("test output coordinate"))
                .collect::<Vec<_>>();
            assert_eq!(coordinates.len(), 4, "primary x,y and secondary x,y");
            [
                (coordinates[0], coordinates[1]),
                (coordinates[2], coordinates[3]),
            ]
        })
        .unwrap_or([(0, 0), (default_x, 0)]);
    if std::env::var_os("HALLEY_TEST_RAW_OUTPUT_LAYOUT").is_none() {
        let origin =
            crate::backend::output_layout::LayoutOrigin::for_locations(positions.map(Point::from));
        positions = positions.map(|position| {
            let point = origin
                .effective(position.into())
                .expect("valid test output layout");
            (point.x, point.y)
        });
    }
    let primary = app.driver.backend.output().clone();
    primary.change_current_state(None, None, None, Some(positions[0].into()));
    app.wayland.space.map_output(&primary, positions[0]);
    output.change_current_state(Some(mode), None, None, Some(positions[1].into()));
    output.set_preferred(mode);
    app.wayland.ensure_output_global::<App>(&output);
    app.wayland.space.map_output(&output, positions[1]);
    app.cameras.insert(output.name(), mode.size);
    app.driver.extra_outputs.push(output);
    app.pointer.set_position((
        f64::from(positions[0].0) + f64::from(default_x) * 0.5,
        f64::from(positions[0].1) + f64::from(app.driver.backend.logical_size().h) * 0.5,
    ));

    let socket = UnixDatagram::bind(path).unwrap();
    socket.set_nonblocking(true).unwrap();
    handle.insert_source(Generic::new(socket, Interest::READ, Mode::Level), move |_, socket, app| {
        let mut bytes = [0; 256];
        while let Ok((size, peer)) = socket.recv_from(&mut bytes) {
            let mut layout_error = None;
            let command = std::str::from_utf8(&bytes[..size]).unwrap();
            let parts = command.split_whitespace().collect::<Vec<_>>();
            if matches!(parts.first(), Some(&"key") | Some(&"seat-key")) {
                let seat_key = parts[0] == "seat-key";
                assert_eq!(parts.len(), if seat_key { 4 } else { 3 });
                let code = parts[1].parse().unwrap();
                let pressed = parts[2] == "down";
                let count = if seat_key { parts[3].parse().unwrap() } else { u32::from(pressed) };
                let event = TestKey { code, pressed, count, time: app.start_time.elapsed().as_millis() as u32 };
                let socket_name = app.wayland_display.clone().unwrap();
                // Winit has one host keyboard stream. Explicit seat-key events
                // instead model the native backend's libinput holder count.
                if !seat_key || crate::input::devices::seat_key_is_transition(event.state(), event.count()) {
                    super::super::input::handle::<_, TestInput>(app, &InputEvent::Keyboard { event }, &socket_name);
                }
            } else if parts.first() == Some(&"motion") {
                assert_eq!(parts.len(), 3);
                let target = (parts[1].parse::<f64>().unwrap(), parts[2].parse::<f64>().unwrap());
                let current = app.pointer.position();
                let event = TestMotion { dx: target.0 - current.0, dy: target.1 - current.1, time: app.start_time.elapsed().as_millis() as u32 };
                let socket_name = app.wayland_display.clone().unwrap();
                super::super::input::handle::<_, TestInput>(app, &InputEvent::PointerMotion { event }, &socket_name);
            } else if parts.first() == Some(&"button") {
                assert_eq!(parts.len(), 3);
                let event = TestButton { code: parts[1].parse().unwrap(), pressed: parts[2] == "down", time: app.start_time.elapsed().as_millis() as u32 };
                let socket_name = app.wayland_display.clone().unwrap();
                super::super::input::handle::<_, TestInput>(app, &InputEvent::PointerButton { event }, &socket_name);
            } else if parts.first() == Some(&"tablet-add") || parts.first() == Some(&"tablet-remove") {
                assert_eq!(parts.len(), 2);
                let device = TestDevice::Tablet(parts[1].parse().unwrap());
                let event = if parts[0] == "tablet-add" { InputEvent::DeviceAdded { device } } else { InputEvent::DeviceRemoved { device } };
                let socket_name = app.wayland_display.clone().unwrap();
                super::super::input::handle::<_, TestInput>(app, &event, &socket_name);
            } else if parts.first() == Some(&"tablet-proximity") || parts.first() == Some(&"tablet-axis") {
                // id tool x y pressure state; coordinates and pressure are normalized.
                assert_eq!(parts.len(), 7);
                let event = TestTablet {
                    device: parts[1].parse().unwrap(), tool: parts[2].parse().unwrap(),
                    x: parts[3].parse().unwrap(), y: parts[4].parse().unwrap(),
                    pressure: parts[5].parse().unwrap(),
                    proximity: match parts[6] { "in" => ProximityState::In, "out" => ProximityState::Out, _ => panic!("invalid proximity") },
                    time: app.start_time.elapsed().as_millis() as u32,
                };
                assert!((0.0..=1.0).contains(&event.x) && (0.0..=1.0).contains(&event.y) && (0.0..=1.0).contains(&event.pressure));
                let socket_name = app.wayland_display.clone().unwrap();
                let input = if parts[0] == "tablet-axis" { InputEvent::TabletToolAxis { event } } else { InputEvent::TabletToolProximity { event } };
                super::super::input::handle::<_, TestInput>(app, &input, &socket_name);
            } else if parts.first() == Some(&"layout") {
                use crate::session::state::OutputDriver;
                assert_eq!(parts.len(), 5);
                let coordinates = parts[1..].iter().map(|part| part.parse::<i32>().unwrap()).collect::<Vec<_>>();
                let requested = app.driver.output_states().into_iter().enumerate().map(|(index,state)| {
                    super::super::output::OutputConfiguration {
                        output: state.output, enabled: true, mode: state.mode,
                        location: (coordinates[index*2], coordinates[index*2+1]).into(),
                        transform: state.transform, scale: state.scale, adaptive_sync: false,
                    }
                }).collect::<Vec<_>>();
                layout_error = app.apply_wayland_output_configuration(&requested).err();
                if layout_error.is_none() { app.notify_output_management(); }
            } else { assert_eq!(command, "state"); }
            let now = crate::frame_clock::monotonic_now();
            let windows = app.nodes.records().map(|record| {
                let output = app.wayland.space.outputs().find(|o| o.name() == record.output).unwrap();
                let geometry = app.wayland.space.output_geometry(output).unwrap();
                let view = app.cameras.view(&record.output).unwrap();
                let viewport = crate::presentation::camera::world_viewport(view, geometry);
                let client = app.wayland.space.element_geometry(&record.window).unwrap_or(record.geometry);
                let outer = crate::titlebar::outer_rect_for_client(&record.window, client, &app.settings.decorations, &app.settings.font);
                serde_json::json!({"id": record.id.as_u64(), "fullscreen": app.fullscreen.is_fullscreen_or_pending(&record.surface),
                    "fullscreen_configure_pending": app.fullscreen.awaits_external_configure(&record.surface),
                    "fullscreen_snapshot": app.render.fullscreen_textures.awaiting_target(&record.surface),
                    "maximized": app.maximize.contains(&record.surface),
                    "animating": app.fullscreen.is_animating_on_output(output, now)
                        || app.maximize.is_animating_on_output(output, crate::presentation::active_workspace_on_output(&app.clusters, &record.output, now), now),
                    "camera_settled": app.cameras.get(&record.output).is_some_and(|camera|
                        camera.center == camera.target_center && camera.view_size == camera.target_view_size),
                    "view": [view.center.x, view.center.y, view.scale],
                    "viewport": [viewport.loc.x, viewport.loc.y, viewport.size.w, viewport.size.h],
                    "client_geometry": [client.loc.x, client.loc.y, client.size.w, client.size.h],
                    "outer_geometry": [outer.loc.x, outer.loc.y, outer.size.w, outer.size.h],
                    "paused": app.fullscreen.is_presentation_paused(&record.surface),
                    "camera": app.fullscreen.camera_frame(output, geometry, now).is_some(),
                    "rect": super::super::presented_window_rect(app, &record.window, output, now).map(|r| [r.loc.x,r.loc.y,r.size.w,r.size.h])})
            }).collect::<Vec<_>>();
            let leds = app.seat.get_keyboard().unwrap().led_state();
            let keyboard = app.seat.get_keyboard().unwrap();
            let modifiers = keyboard.modifier_state();
            let sides = app.keyboard.side_modifiers;
            let last_led_update = app.driver.keyboard_led_update.map(|leds| serde_json::json!({"caps": leds.caps, "num": leds.num, "scroll": leds.scroll}));
            let capture_region = match app.capture.overlay() {
                crate::capture::CaptureOverlay::Region(rect) => Some([rect.loc.x, rect.loc.y, rect.size.w, rect.size.h]),
                _ => None,
            };
            let primary_geometry = app.wayland.space.output_geometry(&primary).unwrap();
            let reply = serde_json::json!({"primary_geometry": [primary_geometry.loc.x, primary_geometry.loc.y, primary_geometry.size.w, primary_geometry.size.h], "layout_error": layout_error, "open": app.shell.focus_cycle.is_open(), "active": app.shell.focus_cycle.is_active(),
                "empty_cluster": app.wayland.space.outputs().filter_map(|output| {
                    let snapshot = app.shell.overlays.snapshot(&output.name(), now).empty_cluster?;
                    Some(serde_json::json!({"output": output.name(), "shortcut": snapshot.shortcut, "armed": snapshot.armed}))
                }).collect::<Vec<_>>(),
                "composer": app.shell.cluster_composer.session().map(|composer| serde_json::json!({
                    "interactive": app.shell.cluster_composer.accepts_input()
                        && composer.progress(now) >= 1.0,
                    "tiles": composer.tiles.iter().map(|tile| serde_json::json!({"id": tile.id.as_u64(),
                        "rect": [tile.target.loc.x, tile.target.loc.y, tile.target.size.w, tile.target.size.h]})).collect::<Vec<_>>(),
                })),
                "notices": app.wayland.space.outputs().filter_map(|output| {
                    let origin = output.current_location();
                    let (target, rect) = app.shell.overlays.notice_card(&output.name())?;
                    Some(serde_json::json!({"output": output.name(), "target": target.as_u64(),
                        "rect": [origin.x + rect.loc.x, origin.y + rect.loc.y, rect.size.w, rect.size.h]}))
                }).collect::<Vec<_>>(),
                "bearings": app.wayland.space.outputs().map(|output| {
                    let origin = output.current_location();
                    let chips = app.shell.bearings.hitboxes(&output.name()).into_iter().filter_map(|hitbox| match hitbox.target {
                        crate::shell::bearings::BearingTarget::Node(id) => Some(serde_json::json!({"id": id.as_u64(),
                            "rect": [origin.x + hitbox.rect.loc.x, origin.y + hitbox.rect.loc.y, hitbox.rect.size.w, hitbox.rect.size.h]})),
                        crate::shell::bearings::BearingTarget::ClusterCore { .. } => None,
                    }).collect::<Vec<_>>();
                    serde_json::json!({"output": output.name(), "mix": app.shell.bearings.mix(&output.name()), "chips": chips})
                }).collect::<Vec<_>>(),
                "capture_active": app.capture.is_active(), "capture_source": app.capture.source_accessibility().2, "capture_region": capture_region,
                "leds": {"caps": leds.caps, "num": leds.num, "scroll": leds.scroll},
                "led_update_count": app.driver.keyboard_led_update_count, "last_led_update": last_led_update,
                "keyboard": {"pressed": keyboard.pressed_keys().into_iter().map(|key| key.raw() - 8).collect::<Vec<_>>(),
                    "locks": [modifiers.caps_lock, modifiers.num_lock], "locked": modifiers.serialized.locked,
                    "modifiers": [modifiers.shift, modifiers.ctrl, modifiers.alt, modifiers.logo],
                    "sides": [sides.left_shift, sides.right_shift, sides.left_ctrl, sides.right_ctrl,
                        sides.left_alt, sides.right_alt, sides.left_super, sides.right_super]},
                "preview": app.shell.focus_cycle.session().and_then(|s| s.preview()).map(|id| id.as_u64()),
                "focused": app.nodes.focused().map(|id| id.as_u64()), "windows": windows,
                "selected_output": app.wayland.focused_output,
                "pointer": app.pointer.position(), "moving": matches!(*app.interactions.grab(), crate::input::grab::Grab::MoveWindow { .. }),
                "pending_move": matches!(*app.interactions.grab(), crate::input::grab::Grab::PendingWindowMove(_)),
                "resizing": matches!(*app.interactions.grab(), crate::input::grab::Grab::ResizeWindow(_))});
            socket.send_to(reply.to_string().as_bytes(), peer.as_pathname().unwrap()).unwrap();
        }
        Ok(PostAction::Continue)
    }).unwrap();
}

#[derive(Debug)]
struct TestInput;
#[derive(Debug)]
struct TestMotion {
    dx: f64,
    dy: f64,
    time: u32,
}
impl Event<TestInput> for TestMotion {
    fn time(&self) -> InputTime {
        InputTime::from_millis(self.time)
    }
    fn device(&self) -> TestDevice {
        TestDevice::Winit
    }
}
impl PointerMotionEvent<TestInput> for TestMotion {
    fn delta_x(&self) -> f64 {
        self.dx
    }
    fn delta_y(&self) -> f64 {
        self.dy
    }
    fn delta_x_unaccel(&self) -> f64 {
        self.dx
    }
    fn delta_y_unaccel(&self) -> f64 {
        self.dy
    }
}
#[derive(Debug)]
struct TestButton {
    code: u32,
    pressed: bool,
    time: u32,
}
impl Event<TestInput> for TestButton {
    fn time(&self) -> InputTime {
        InputTime::from_millis(self.time)
    }
    fn device(&self) -> TestDevice {
        TestDevice::Winit
    }
}
impl PointerButtonEvent<TestInput> for TestButton {
    fn button_code(&self) -> u32 {
        self.code
    }
    fn state(&self) -> ButtonState {
        if self.pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        }
    }
}
#[derive(Debug)]
struct TestKey {
    code: u32,
    pressed: bool,
    time: u32,
    count: u32,
}
impl Event<TestInput> for TestKey {
    fn time(&self) -> InputTime {
        InputTime::from_millis(self.time)
    }
    fn device(&self) -> TestDevice {
        TestDevice::Winit
    }
}
impl KeyboardKeyEvent<TestInput> for TestKey {
    fn key_code(&self) -> Keycode {
        Keycode::new(self.code + 8)
    }
    fn state(&self) -> KeyState {
        if self.pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        }
    }
    fn count(&self) -> u32 {
        self.count
    }
}
impl InputBackend for TestInput {
    type Device = TestDevice;
    type KeyboardKeyEvent = TestKey;
    type PointerAxisEvent = UnusedEvent;
    type PointerButtonEvent = TestButton;
    type PointerMotionEvent = TestMotion;
    type PointerMotionAbsoluteEvent = UnusedEvent;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = TestTablet;
    type TabletToolProximityEvent = TestTablet;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = UnusedEvent;
}

// The winit stream retains its original capabilities; synthetic tablets have
// stable distinct identities, without falsely presenting as mice or keyboards.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TestDevice {
    Winit,
    Tablet(u32),
}
impl Device for TestDevice {
    fn id(&self) -> String {
        match self {
            Self::Winit => WinitVirtualDevice.id(),
            Self::Tablet(id) => format!("test-tablet-{id}"),
        }
    }
    fn name(&self) -> String {
        match self {
            Self::Winit => WinitVirtualDevice.name(),
            Self::Tablet(id) => format!("Test tablet {id}"),
        }
    }
    fn has_capability(&self, capability: DeviceCapability) -> bool {
        match self {
            Self::Winit => WinitVirtualDevice.has_capability(capability),
            Self::Tablet(_) => capability == DeviceCapability::TabletTool,
        }
    }
    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }
    fn syspath(&self) -> Option<std::path::PathBuf> {
        None
    }
}
#[derive(Debug)]
struct TestTablet {
    device: u32,
    tool: u64,
    x: f64,
    y: f64,
    pressure: f64,
    proximity: ProximityState,
    time: u32,
}
impl Event<TestInput> for TestTablet {
    fn time(&self) -> InputTime {
        InputTime::from_millis(self.time)
    }
    fn device(&self) -> TestDevice {
        TestDevice::Tablet(self.device)
    }
}
impl AbsolutePositionEvent<TestInput> for TestTablet {
    fn x(&self) -> f64 {
        self.x
    }
    fn y(&self) -> f64 {
        self.y
    }
    fn x_transformed(&self, width: i32) -> f64 {
        self.x * f64::from(width)
    }
    fn y_transformed(&self, height: i32) -> f64 {
        self.y * f64::from(height)
    }
}
impl TabletToolEvent<TestInput> for TestTablet {
    fn tool(&self) -> TabletToolDescriptor {
        TabletToolDescriptor {
            tool_type: TabletToolType::Pen,
            hardware_serial: self.tool,
            hardware_id_wacom: 0,
            capabilities: TabletToolCapabilities::PRESSURE,
        }
    }
    fn delta_x(&self) -> f64 {
        0.0
    }
    fn delta_y(&self) -> f64 {
        0.0
    }
    fn distance(&self) -> f64 {
        0.0
    }
    fn distance_has_changed(&self) -> bool {
        false
    }
    fn pressure(&self) -> f64 {
        self.pressure
    }
    fn pressure_has_changed(&self) -> bool {
        true
    }
    fn slider_position(&self) -> f64 {
        0.0
    }
    fn slider_has_changed(&self) -> bool {
        false
    }
    fn tilt_x(&self) -> f64 {
        0.0
    }
    fn tilt_x_has_changed(&self) -> bool {
        false
    }
    fn tilt_y(&self) -> f64 {
        0.0
    }
    fn tilt_y_has_changed(&self) -> bool {
        false
    }
    fn rotation(&self) -> f64 {
        0.0
    }
    fn rotation_has_changed(&self) -> bool {
        false
    }
    fn wheel_delta(&self) -> f64 {
        0.0
    }
    fn wheel_delta_discrete(&self) -> i32 {
        0
    }
    fn wheel_has_changed(&self) -> bool {
        false
    }
}
impl TabletToolAxisEvent<TestInput> for TestTablet {}
impl TabletToolProximityEvent<TestInput> for TestTablet {
    fn state(&self) -> ProximityState {
        self.proximity
    }
}
