use smithay::backend::input::{
    ButtonState, Device, DeviceCapability, Event, InputBackend, InputEvent, ProximityState,
    TabletToolButtonEvent, TabletToolEvent, TabletToolProximityEvent, TabletToolTipEvent,
    TabletToolTipState,
};
use smithay::input::tablet::{TabletDescriptor, TabletSeatTrait, tool};
use smithay::reexports::wayland_server::Resource;
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
            let had_cursor = session.tablet_proximity.has_cursor();
            let tools = session.tablet_proximity.cancel_device(&device.id());
            if had_cursor && !session.tablet_proximity.has_cursor() {
                super::pointer::reset_client_cursor_image(session);
                session.request_redraw();
            }
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
    if session.tablet_proximity.has_cursor() {
        super::pointer::reset_client_cursor_image(session);
        session.request_redraw();
    }
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
        release_pointer_buttons(session, &desc, time);
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

// A pen stroke chooses its protocol at press time. Smithay's pointer click grab
// retains the original focus until release, even when the pen leaves the surface.
fn pointer_motion<D: SessionDriver>(
    session: &mut Session<D>,
    tool: &smithay::input::tablet::tool::TabletToolHandle<Session<D>>,
    desc: &smithay::backend::input::TabletToolDescriptor,
    route: Option<&crate::input::pointer::PointerRoute>,
    position: Option<smithay::utils::Point<f64, smithay::utils::Logical>>,
    time: smithay::backend::input::InputTime,
) -> bool {
    let held = session.tablet_proximity.pointer_held(desc);
    // Do not inject a second device into another pointer or tablet grab.
    if !held && (tool.is_grabbed() || session.pointer_handle().is_grabbed()) {
        return false;
    }
    let pointer = session.pointer_handle();
    let before = session.pointer.position();
    let pointer_before = pointer.current_location();
    if let Some(position) = position {
        session.pointer.set_position(position.into());
    }
    let constrained = super::pointer::constrain_motion(session, &pointer);
    let corrected_route = match constrained {
        super::pointer::ConstrainedMotion::RelativeOnly { surface, origin } => {
            session.pointer.set_position(before);
            pointer.set_location(pointer_before);
            if let Some(position) = position {
                let delta = position - smithay::utils::Point::from(before);
                pointer.relative_motion(
                    session,
                    Some((surface, origin)),
                    &smithay::input::pointer::RelativeMotionEvent {
                        delta,
                        delta_unaccel: delta,
                        time,
                    },
                );
                pointer.frame(session);
            }
            return held || pointer.current_focus().is_some();
        }
        super::pointer::ConstrainedMotion::Clamp(position) => {
            session.pointer.set_position(position.into());
            super::touch::route(session, position)
        }
        super::pointer::ConstrainedMotion::Apply => position.and_then(|proposed| {
            let effective = smithay::utils::Point::from(session.pointer.position());
            (effective != proposed)
                .then(|| super::touch::route(session, effective))
                .flatten()
        }),
    };
    let route = corrected_route.as_ref().or(route);
    let confined = super::pointer::confined_focus(session, &pointer);
    let focus = confined
        .clone()
        .or_else(|| route.and_then(|route| route.focus.clone()));
    let eligible = focus
        .as_ref()
        .and_then(|(surface, _)| surface.client())
        .is_some_and(|client| held || tool.client_tools(&client).next().is_none());
    let location = if confined.is_some() {
        pointer.current_location()
    } else {
        route.map_or_else(|| pointer.current_location(), |route| route.location)
    };
    pointer.motion(
        session,
        if held || eligible { focus } else { None },
        &smithay::input::pointer::MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    pointer.frame(session);
    held || eligible
}

fn pointer_button<D: SessionDriver>(
    session: &mut Session<D>,
    desc: &smithay::backend::input::TabletToolDescriptor,
    button: u32,
    state: ButtonState,
    time: smithay::backend::input::InputTime,
    eligible: bool,
) {
    let send = match state {
        ButtonState::Pressed if eligible => {
            session.tablet_proximity.pointer_press(desc.clone(), button)
        }
        ButtonState::Released => session.tablet_proximity.pointer_release(desc, button),
        _ => false,
    };
    if send {
        let pointer = session.pointer_handle();
        pointer.button(
            session,
            &smithay::input::pointer::ButtonEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time,
                button,
                state,
            },
        );
        pointer.frame(session);
    }
}

fn release_pointer_buttons<D: SessionDriver>(
    session: &mut Session<D>,
    desc: &smithay::backend::input::TabletToolDescriptor,
    time: smithay::backend::input::InputTime,
) {
    for button in session.tablet_proximity.take_pointer_buttons(desc) {
        let pointer = session.pointer_handle();
        pointer.button(
            session,
            &smithay::input::pointer::ButtonEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time,
                button,
                state: ButtonState::Released,
            },
        );
        pointer.frame(session);
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

// Tablet position is a compositor cursor position, not synthetic wl_pointer input.
fn update_cursor<D: SessionDriver>(
    session: &mut Session<D>,
    descriptor: &smithay::backend::input::TabletToolDescriptor,
    position: Option<smithay::utils::Point<f64, smithay::utils::Logical>>,
    focus: Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>,
) {
    let Some(position) = position else {
        if session.tablet_proximity.owns_cursor(descriptor)
            && session.tablet_cursor_focus.take().is_some()
        {
            super::pointer::reset_client_cursor_image(session);
            session.request_redraw();
        }
        return;
    };
    let position = if super::pointer::has_active_constraint(session)
        && !super::pointer::constraint_suspended_for_grab(session)
    {
        session.pointer.position().into()
    } else {
        position
    };
    let changed_owner = !session.tablet_proximity.owns_cursor(descriptor);
    session.tablet_proximity.cursor_activity(descriptor);
    if changed_owner || session.tablet_cursor_focus != focus {
        super::pointer::reset_client_cursor_image(session);
    }
    session.tablet_cursor_focus = focus;
    session.pointer.set_position(position.into());
    session.cursor_policy.pointer_activity();
    crate::cursor::surface::refresh_outputs(
        &session.cursor,
        &session.wayland.space,
        session.pointer.position(),
    );
    session.request_redraw();
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
                    let owned_cursor = session.tablet_proximity.owns_cursor(&descriptor);
                    if session.tablet_proximity.leave(&descriptor) {
                        release_pointer_buttons(session, &descriptor, event.time());
                        if owned_cursor {
                            super::pointer::reset_client_cursor_image(session);
                            session.request_redraw();
                        }
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
                    pointer_motion(
                        session,
                        &tool,
                        &descriptor,
                        route.as_ref(),
                        position,
                        event.time(),
                    );
                    // Keep the physical tool in proximity without focusing a surface;
                    // a later positioned event can then establish focus.
                    let (focus, location) = route
                        .map_or((None, position.unwrap_or_default()), |route| {
                            (route.focus, route.location)
                        });
                    let cursor_focus = focus.as_ref().map(|(surface, _)| surface.clone());
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
                        .enter(descriptor.clone(), event.device().id());
                    update_cursor(session, &descriptor, position, cursor_focus);
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
            pointer_motion(
                session,
                &tool,
                &descriptor,
                route.as_ref(),
                position,
                event.time(),
            );
            let (focus, location) = route.map_or((None, position.unwrap_or_default()), |route| {
                (route.focus, route.location)
            });
            let cursor_focus = focus.as_ref().map(|(surface, _)| surface.clone());
            tool.motion(
                session,
                focus,
                &tool::MotionEvent {
                    location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time(),
                },
            );
            update_cursor(session, &descriptor, position, cursor_focus);
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
                    if matches!(event.tip_state(), TabletToolTipState::Down)
                        && !super::pointer::has_active_constraint(session)
                        && (!session.pointer_handle().is_grabbed()
                            || session.tablet_proximity.pointer_held(&descriptor))
                        && !tool.is_grabbed()
                    {
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
                    let eligible = pointer_motion(
                        session,
                        &tool,
                        &descriptor,
                        route.as_ref(),
                        Some(position),
                        event.time(),
                    );
                    let state = match event.tip_state() {
                        TabletToolTipState::Down => ButtonState::Pressed,
                        TabletToolTipState::Up => ButtonState::Released,
                    };
                    pointer_button(session, &descriptor, 272, state, event.time(), eligible);
                    let (focus, location) =
                        route.map_or((None, position), |route| (route.focus, route.location));
                    let cursor_focus = focus.as_ref().map(|(surface, _)| surface.clone());
                    tool.motion(
                        session,
                        focus,
                        &tool::MotionEvent {
                            location,
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time(),
                        },
                    );
                    update_cursor(session, &descriptor, Some(position), cursor_focus);
                    tool.axis(session, axis::<B, _>(event));
                } else {
                    if matches!(event.tip_state(), TabletToolTipState::Up) {
                        pointer_button(
                            session,
                            &descriptor,
                            272,
                            ButtonState::Released,
                            event.time(),
                            false,
                        );
                    }
                    update_cursor(session, &descriptor, None, None);
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
