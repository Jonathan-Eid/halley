use smithay::backend::input::{
    Device, DeviceCapability, Event, InputBackend, InputEvent, ProximityState,
    TabletToolButtonEvent, TabletToolEvent, TabletToolProximityEvent, TabletToolTipEvent,
    TabletToolTipState,
};
use smithay::input::tablet::{TabletDescriptor, TabletSeatTrait, tool};
use smithay::utils::SERIAL_COUNTER;

use super::{Session, SessionDriver};

pub(super) fn device_event<D: SessionDriver, B: InputBackend>(
    session: &mut Session<D>,
    event: &InputEvent<B>,
) -> bool {
    match event {
        InputEvent::DeviceAdded { device }
            if device.has_capability(DeviceCapability::TabletTool) =>
        {
            let seat = session.seat.tablet_seat();
            let desc = TabletDescriptor::from(device);
            if seat.get_tablet(&desc).is_none() {
                seat.add_wp_tablet(&session.wayland.display_handle, &desc);
            }
        }
        InputEvent::DeviceRemoved { device }
            if device.has_capability(DeviceCapability::TabletTool) =>
        {
            let tools = session.tablet_proximity.cancel_device(&device.id());
            cancel_tools(session, tools);
            let seat = session.seat.tablet_seat();
            seat.remove_tablet(&TabletDescriptor::from(device));
            if seat.count_tablets() == 0 {
                seat.clear_tools();
            }
        }
        _ => {}
    }
    false
}

pub(crate) fn cancel_all<D: SessionDriver>(session: &mut Session<D>) {
    let tools = session.tablet_proximity.cancel_all();
    cancel_tools(session, tools);
}

fn cancel_tools<D: SessionDriver>(
    session: &mut Session<D>,
    tools: Vec<smithay::backend::input::TabletToolDescriptor>,
) {
    let seat = session.seat.tablet_seat();
    let time = smithay::backend::input::InputTime::from_millis(
        session.start_time.elapsed().as_millis() as u32,
    );
    for desc in tools {
        if let Some(tool) = seat.get_tool(&desc) {
            tool.proximity_out(
                session,
                &tool::ProximityOutEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            tool.frame(session, time);
        }
    }
}

fn tablet_position<D, B, E>(
    session: &Session<D>,
    event: &E,
) -> Option<smithay::utils::Point<f64, smithay::utils::Logical>>
where
    D: SessionDriver,
    B: InputBackend,
    E: smithay::backend::input::AbsolutePositionEvent<B>,
{
    let settings = session
        .settings
        .input
        .settings_for_device(halley_config::DeviceKind::Tablet, &event.device().name());
    let output = settings
        .map_to_output
        .as_deref()
        .and_then(|name| session.wayland.space.outputs().find(|o| o.name() == name))
        .unwrap_or_else(|| session.driver.primary_output());
    super::touch::transform_absolute_position(
        event,
        output,
        session.wayland.space.output_geometry(output)?,
    )
}

fn axis<B: InputBackend, E: TabletToolEvent<B>>(event: &E) -> tool::AxisFrame {
    tool::AxisFrame {
        pressure: event.pressure_has_changed().then(|| event.pressure()),
        distance: event.distance_has_changed().then(|| event.distance()),
        tilt: event.tilt_has_changed().then(|| event.tilt()),
        rotation: event.rotation_has_changed().then(|| event.rotation()),
        slider: event.slider_has_changed().then(|| event.slider_position()),
        wheel: event
            .wheel_has_changed()
            .then(|| (event.wheel_delta(), event.wheel_delta_discrete())),
    }
}

pub(super) fn handle<D: SessionDriver, B: InputBackend>(
    session: &mut Session<D>,
    event: &InputEvent<B>,
) -> bool {
    if (session.capture.is_active()
        || session.shell.overlays.confirmation_modal_active()
        || session.shell.cluster_composer.is_active())
        && matches!(
            event,
            InputEvent::TabletToolProximity { .. }
                | InputEvent::TabletToolAxis { .. }
                | InputEvent::TabletToolTip { .. }
                | InputEvent::TabletToolButton { .. }
        )
    {
        return true;
    }
    let seat = session.seat.tablet_seat();
    match event {
        InputEvent::TabletToolProximity { event } => {
            let descriptor = event.tool();
            match event.state() {
                ProximityState::Out => {
                    if session.tablet_proximity.leave(&descriptor) {
                        if let Some(tool) = seat.get_tool(&descriptor) {
                            tool.proximity_out(
                                session,
                                &tool::ProximityOutEvent {
                                    serial: SERIAL_COUNTER.next_serial(),
                                    time: event.time(),
                                },
                            );
                            tool.frame(session, event.time());
                        }
                    }
                }
                ProximityState::In => {
                    let position = tablet_position::<D, B, _>(session, event);
                    let Some(tablet) = seat.get_tablet(&TabletDescriptor::from(&event.device()))
                    else {
                        return true;
                    };
                    let route =
                        position.and_then(|position| super::touch::route(session, position));
                    let tool = seat.get_tool(&descriptor).unwrap_or_else(|| {
                        let display = session.wayland.display_handle.clone();
                        seat.add_wp_tool(session, &display, &descriptor)
                    });
                    // Keep the physical tool in proximity without focusing a surface;
                    // a later positioned event can then establish focus.
                    let (focus, location) = route
                        .map_or((None, position.unwrap_or_default()), |route| {
                            (route.focus, route.location)
                        });
                    tool.proximity_in(
                        session,
                        focus,
                        tablet,
                        &tool::ProximityInEvent {
                            location,
                            axis: Some(axis::<B, _>(event)),
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time(),
                        },
                    );
                    session
                        .tablet_proximity
                        .enter(descriptor, event.device().id());
                    tool.frame(session, event.time());
                }
            }
        }
        InputEvent::TabletToolAxis { event } => {
            let descriptor = event.tool();
            if !session.tablet_proximity.contains(&descriptor) {
                return true;
            }
            let Some(tool) = seat.get_tool(&descriptor) else {
                return true;
            };
            let position = tablet_position::<D, B, _>(session, event);
            let route = position.and_then(|position| super::touch::route(session, position));
            let (focus, location) = route.map_or((None, position.unwrap_or_default()), |route| {
                (route.focus, route.location)
            });
            tool.motion(
                session,
                focus,
                &tool::MotionEvent {
                    location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time(),
                },
            );
            tool.axis(session, axis::<B, _>(event));
            tool.frame(session, event.time());
        }
        InputEvent::TabletToolTip { event } => {
            let descriptor = event.tool();
            if !session.tablet_proximity.contains(&descriptor) {
                return true;
            }
            if let Some(tool) = seat.get_tool(&descriptor) {
                if let Some(position) = tablet_position::<D, B, _>(session, event) {
                    let route = super::touch::route(session, position);
                    if matches!(event.tip_state(), TabletToolTipState::Down) {
                        if let Some(route) = &route {
                            crate::wayland::focus::select_output(
                                &mut session.wayland,
                                &route.output,
                            );
                            let serial = SERIAL_COUNTER.next_serial();
                            match &route.target {
                                crate::input::pointer::PointerTarget::Window(window)
                                | crate::input::pointer::PointerTarget::Decoration {
                                    window, ..
                                } => {
                                    super::focus::focus_window_from_pointer(session, window, serial)
                                }
                                crate::input::pointer::PointerTarget::Layer(layer) => {
                                    super::focus::focus_layer(session, Some(layer.clone()), serial)
                                }
                                crate::input::pointer::PointerTarget::Background => {}
                            }
                        }
                    }
                    let (focus, location) =
                        route.map_or((None, position), |route| (route.focus, route.location));
                    tool.motion(
                        session,
                        focus,
                        &tool::MotionEvent {
                            location,
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time(),
                        },
                    );
                    tool.axis(session, axis::<B, _>(event));
                } else {
                    tool.motion(
                        session,
                        None,
                        &tool::MotionEvent {
                            location: Default::default(),
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time(),
                        },
                    );
                }
                let serial = SERIAL_COUNTER.next_serial();
                match event.tip_state() {
                    TabletToolTipState::Down => tool.down(
                        session,
                        &tool::DownEvent {
                            serial,
                            time: event.time(),
                        },
                    ),
                    TabletToolTipState::Up => tool.up(
                        session,
                        &tool::UpEvent {
                            serial,
                            time: event.time(),
                        },
                    ),
                }
                tool.frame(session, event.time());
            }
        }
        InputEvent::TabletToolButton { event } => {
            let descriptor = event.tool();
            if !session.tablet_proximity.contains(&descriptor) {
                return true;
            }
            if let Some(tool) = seat.get_tool(&descriptor) {
                tool.button(
                    session,
                    &tool::ButtonEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        button: event.button(),
                        state: event.button_state(),
                        time: event.time(),
                    },
                );
                tool.frame(session, event.time());
            }
        }
        _ => return false,
    }
    true
}
