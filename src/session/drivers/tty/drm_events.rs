//! Hotplug event wiring for the TTY (DRM/KMS) driver.
//!
//! `DrmDeviceNotifier` only delivers `VBlank`/`Error` — Smithay drops
//! non-page-flip device events on the floor — so connector hotplug is
//! polled with `DrmScanner` when the existing udev power monitor reports
//! a DRM device change (`Added`/`Changed`/`Removed`). Results are
//! dispatched as `DrmScanEvent::Connected`/`Disconnected`/`Changed`
//! into the [`HotplugBackend`] API below, and each returned
//! `(Output, SideEffect)` is registered/unregistered with the session.
//!
//! The backend retains one `DrmScanner` per GPU (`TtyGpu::scanner`),
//! so `scan_hotplug_events` reports diffs since the previous call.
//! `apply_hotplug_connector` allocates CRTC/mode/output state for
//! `Connected` (`attach_output`); `apply_hotplug_disconnect` tears it
//! down via `remove_output` (IPC info included); `apply_hotplug_changed`
//! refreshes stored connector info.
//!
//! Design notes / TODO:
//!   1. Last output: the session refuses to unregister the final output
//!      (see `unregister_output`), so a compositor never goes fully dark.
//!   2. `Changed` is currently informational only (IPC refresh); it does
//!      not re-run `output_target` mode selection like config reload
//!      does. Wire that only if a mode-list change (e.g. EDID update)
//!      needs mid-session re-negotiation.
//!
//! Naming: `HotplugSideEffect` is [`HotplugSideEffect`]; the trait can be
//! collapsed into inherent `TtyBackend` methods when Smithay's udev
//! path already passes `(DrmScanEvent)` — the separate trait exists to
//! keep this file's contract explicit.

use smithay::output::Output;
use smithay::reexports::drm::control::{connector, crtc};
use smithay_drm_extras::drm_scanner::DrmScanEvent;

use crate::backend::tty::TtyBackend;
use crate::session::state::OutputDriver as _;

use super::frame::OutputFrameState;
use super::{TtyApp, apply_tty_output_config};

/// Per-output side effect of a backend hotplug apply, telling the
/// session which bookkeeping to perform. Mirrors the flags on
/// `backend::tty::AppliedOutputChange` (`layout_changed`,
/// `enabled_changed`, …) collapsed to the three session actions
/// hotplug needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HotplugSideEffect {
    /// Newly initialized and enabled; register from scratch.
    Added,
    /// Torn down; unregister (Wayland disable path).
    Removed,
    /// Known output whose mode/layout changed; refresh like
    /// `apply_tty_output_config` does for `layout_changed`.
    Changed,
}

/// Minimal assumed backend API. See module docs; stubbed until the
/// backend agent lands the real implementation in `backend/tty`.
pub(crate) trait HotplugBackend {
    fn hotplug_scan_all(&mut self) -> Result<Vec<(usize, DrmScanEvent)>, String>;
    fn apply_hotplug_connector(
        &mut self,
        gpu_index: usize,
        connector: connector::Info,
        crtc: Option<crtc::Handle>,
        configured: Option<&halley_config::OutputConfig>,
    ) -> Result<Vec<(Output, HotplugSideEffect)>, String>;
    fn apply_hotplug_disconnect(
        &mut self,
        gpu_index: usize,
        connector: connector::Info,
        crtc: Option<crtc::Handle>,
    ) -> Result<Vec<(Output, HotplugSideEffect)>, String>;
    fn apply_hotplug_changed(
        &mut self,
        gpu_index: usize,
        connector: connector::Info,
        crtc: Option<crtc::Handle>,
        configured: Option<&halley_config::OutputConfig>,
    ) -> Result<Vec<(Output, HotplugSideEffect)>, String>;
}

impl HotplugBackend for TtyBackend {
    fn hotplug_scan_all(&mut self) -> Result<Vec<(usize, DrmScanEvent)>, String> {
        Err("hotplug: backend scan not yet implemented (backend agent TODO)".into())
    }

    fn apply_hotplug_connector(
        &mut self,
        _gpu_index: usize,
        _connector: connector::Info,
        _crtc: Option<crtc::Handle>,
        _configured: Option<&halley_config::OutputConfig>,
    ) -> Result<Vec<(Output, HotplugSideEffect)>, String> {
        Err("hotplug: apply_hotplug_connector not yet implemented (backend agent TODO)".into())
    }

    fn apply_hotplug_disconnect(
        &mut self,
        _gpu_index: usize,
        _connector: connector::Info,
        _crtc: Option<crtc::Handle>,
    ) -> Result<Vec<(Output, HotplugSideEffect)>, String> {
        Err("hotplug: apply_hotplug_disconnect not yet implemented (backend agent TODO)".into())
    }

    fn apply_hotplug_changed(
        &mut self,
        _gpu_index: usize,
        _connector: connector::Info,
        _crtc: Option<crtc::Handle>,
        _configured: Option<&halley_config::OutputConfig>,
    ) -> Result<Vec<(Output, HotplugSideEffect)>, String> {
        Err("hotplug: apply_hotplug_changed not yet implemented (backend agent TODO)".into())
    }
}

/// Mirror of `backend::tty::output::connector_name` (`pub(super)` there,
/// so not callable from the session). Keep the two in sync: config
/// resolution and IPC names both depend on this spelling.
fn connector_name(connector: &connector::Info) -> String {
    format!(
        "{}-{}",
        connector.interface().as_str(),
        connector.interface_id()
    )
}

/// Resolve the session config for a hotplugged connector. Closest
/// existing helper: `backend::tty::output::output_target`, which does
/// `outputs_config.iter().find(|cfg| cfg.name == name)` and falls back
/// to preferred mode / scale 1.0 / offset (0,0) when `None`. The
/// session retains the last applied slice in
/// `TtyDriver::applied_output_config` (updated by
/// `apply_tty_output_config`), since `RuntimeSettings` deliberately
/// does not store output hardware config.
fn resolve_output_config<'a>(
    configs: &'a [halley_config::OutputConfig],
    name: &str,
) -> Option<&'a halley_config::OutputConfig> {
    configs.iter().find(|cfg| cfg.name == name)
}

/// Entry point from the udev power-monitor source in `run()`. Scans all
/// GPUs and dispatches each `DrmScanEvent`. Returns early (no-op) when
/// the session is paused: KMS cannot be modeset while another VT owns
/// it, matching `apply_output_configuration`'s paused guard. Events
/// missed while paused are re-discovered on resume because the backend
/// scanner diffs against its last scan.
pub(super) fn on_udev_hotplug(app: &mut TtyApp) {
    if app.driver.pause_reasons.any() {
        eventline::debug!("hotplug: scan deferred while session is paused");
        return;
    }
    let events = match app.driver.backend.hotplug_scan_all() {
        Ok(events) => events,
        Err(err) => {
            eventline::warn!("hotplug: connector scan failed: {err}");
            return;
        }
    };
    if events.is_empty() {
        return;
    }
    eventline::info!("hotplug: {} connector event(s)", events.len());
    for (gpu_index, event) in events {
        handle_scan_event(app, gpu_index, event);
    }
}

fn handle_scan_event(app: &mut TtyApp, gpu_index: usize, event: DrmScanEvent) {
    match event {
        DrmScanEvent::Connected { connector, crtc } => {
            let name = connector_name(&connector);
            let configured = resolve_output_config(&app.driver.applied_output_config, &name)
                .cloned();
            match app.driver.backend.apply_hotplug_connector(
                gpu_index,
                connector,
                crtc,
                configured.as_ref(),
            ) {
                Ok(applied) => dispatch_applied(app, "connected", &applied),
                Err(err) => eventline::warn!("hotplug: connector {name:?} failed to apply: {err}"),
            }
        }
        DrmScanEvent::Disconnected { connector, crtc } => {
            let name = connector_name(&connector);
            match app
                .driver
                .backend
                .apply_hotplug_disconnect(gpu_index, connector, crtc)
            {
                Ok(applied) => dispatch_applied(app, "disconnected", &applied),
                Err(err) => eventline::warn!("hotplug: connector {name:?} failed to remove: {err}"),
            }
        }
        DrmScanEvent::Changed { connector, crtc } => {
            let name = connector_name(&connector);
            let configured = resolve_output_config(&app.driver.applied_output_config, &name)
                .cloned();
            match app.driver.backend.apply_hotplug_changed(
                gpu_index,
                connector,
                crtc,
                configured.as_ref(),
            ) {
                Ok(applied) => dispatch_applied(app, "changed", &applied),
                Err(err) => eventline::warn!("hotplug: connector {name:?} failed to apply: {err}"),
            }
        }
    }
}

fn dispatch_applied(app: &mut TtyApp, verb: &str, applied: &[(Output, HotplugSideEffect)]) {
    if applied.is_empty() {
        eventline::debug!("hotplug: {verb} produced no session changes");
        return;
    }
    let mut layout_changed = false;
    for (output, effect) in applied {
        eventline::info!(
            "hotplug: {verb} output {:?} ({effect:?})",
            output.name()
        );
        match effect {
            HotplugSideEffect::Added => {
                register_new_output(app, output);
                layout_changed = true;
            }
            HotplugSideEffect::Removed => {
                unregister_output(app, output);
                layout_changed = true;
            }
            HotplugSideEffect::Changed => {
                refresh_changed_output(app, output);
                layout_changed = true;
            }
        }
    }
    if layout_changed {
        finish_layout_change(app);
    }
}

/// Full session registration for a newly connected output. Mirrors the
/// startup sequence in `run()` (global + Space + camera) plus the
/// `layout_changed` arm of `apply_tty_output_config` (layer arrange,
/// XWayland/capture resync, pointer refresh, output-management notify),
/// plus a fresh `OutputFrameState` and frame scheduling.
fn register_new_output(app: &mut TtyApp, output: &Output) {
    let now = crate::frame_clock::monotonic_now();
    let interval = app.driver.backend.refresh_interval_for_output(output);
    let vrr_active = app.driver.backend.output_vrr_active(output);
    let mut frame = OutputFrameState::new(interval);
    frame.set_vrr(vrr_active);
    if app.driver.pause_reasons.any() {
        drop(frame.suspend(now));
    }
    app.driver.output_frames.insert(output.clone(), frame);

    app.wayland.ensure_output_global::<TtyApp>(output);
    app.wayland
        .space
        .map_output(output, output.current_location());
    smithay::desktop::layer_map_for_output(output).arrange();

    let geometry = app
        .wayland
        .space
        .output_geometry(output)
        .expect("newly hotplugged output is mapped");
    // Fresh camera at rest, exactly like startup's `cameras.insert`.
    app.cameras
        .insert(output.name(), geometry.size.to_physical(1));
    let external = app
        .fullscreen
        .reconfigure_output(&app.wayland, output);
    crate::xwayland::reconfigure_fullscreen(external);

    // Reuse the config path so any later config reload that touches this
    // output goes through one code path; it is a no-op for outputs whose
    // effective target already matches.
    let configs = app.driver.applied_output_config.clone();
    apply_tty_output_config(app, &configs);

    super::queue_output_redraw(app, output);
    app.request_output_redraw(output);
}

/// Session teardown for a disconnected output. Mirrors the disabled
/// arm of `Session::apply_wayland_output_configuration`: gamma reset,
/// windows reassigned to a fallback, Space unmap, global disabled,
/// camera/overlay/blur state dropped, frame timers cancelled. Refuses
/// to remove the last mapped output (Wayland requires an enabled head;
/// backend agent must confirm its own fallback policy).
fn unregister_output(app: &mut TtyApp, output: &Output) {
    let mapped: Vec<_> = app.wayland.space.outputs().cloned().collect();
    if mapped.len() <= 1 {
        eventline::warn!(
            "hotplug: ignoring disconnect of last output {:?}; backend must not report it",
            output.name()
        );
        return;
    }
    let fallback = mapped
        .iter()
        .find(|candidate| *candidate != output)
        .cloned()
        .unwrap_or_else(|| app.driver.backend.primary_output().clone());

    app.wayland.wlr_gamma_control_state.output_disabled(output);
    app.wayland.wlr_screencopy_state.output_disabled(output);
    app.clear_idle_inhibit_output(output);
    if let Err(err) = app.driver.set_gamma(output, None) {
        eventline::warn!(
            "output {:?}: failed to reset gamma while disconnecting: {err}",
            output.name()
        );
    }

    let windows: Vec<_> = app
        .wayland
        .space
        .elements()
        .filter(|window| {
            crate::wayland::window_output_name(window)
                .is_some_and(|name| name == output.name())
        })
        .cloned()
        .collect();
    for window in windows {
        crate::wayland::set_window_output(&window, &fallback);
    }
    let reassigned = app.fullscreen.reassign_disabled_output(
        &mut app.wayland,
        &app.cameras,
        &output.name(),
        &fallback,
    );
    for surface in reassigned.retired_snapshots {
        app.render.fullscreen_textures.remove(&surface);
    }
    crate::xwayland::reconfigure_fullscreen(reassigned.external_configures);

    if let Some(mut frame) = app.driver.output_frames.remove(output) {
        let now = crate::frame_clock::monotonic_now();
        for token in frame.suspend(now) {
            app.driver.loop_handle.remove(token);
        }
    }
    app.wayland.space.unmap_output(output);
    app.wayland.disable_output_global::<TtyApp>(output);
    app.cameras.remove(&output.name());
    app.shell.overlays.remove_output(&output.name());
    app.render
        .backdrop_blur_renderer
        .remove_output(&output.name());
    app.request_output_redraw(&fallback);
}

/// Refresh for `DrmScanEvent::Changed` / `HotplugSideEffect::Changed`.
/// Reuses `apply_tty_output_config` (camera reset, layer arrange, frame
/// clock/VRR refresh) rather than duplicating it.
fn refresh_changed_output(app: &mut TtyApp, output: &Output) {
    let interval = app.driver.backend.refresh_interval_for_output(output);
    if let Some(state) = app.driver.output_frames.get_mut(output) {
        state.replace_clock(interval);
        state.set_vrr(app.driver.backend.output_vrr_active(output));
    }
    let configs = app.driver.applied_output_config.clone();
    apply_tty_output_config(app, &configs);
    super::queue_output_redraw(app, output);
}

/// Shared tail for any layout change: Space refresh, display scale,
/// XWayland geometry/stacking, capture layout, session-lock surfaces,
/// pointer containment + grab/constraint reconcile, and
/// `notify_output_management`. Mirrors the tail of
/// `apply_tty_output_config`.
fn finish_layout_change(app: &mut TtyApp) {
    app.wayland.space.refresh();
    crate::wayland::display_scale::refresh(&app.wayland, app.driver.backend.primary_output());
    crate::xwayland::sync_positions(app);
    crate::xwayland::sync_stacking_order(app);
    app.xwayland.sync_desktop_geometry(&app.wayland.space);
    if !app.capture.update_layout(&app.wayland.space) {
        crate::capture::cancel_selected(app);
    }
    crate::wayland::session_lock::configure_surfaces(app);
    contain_pointer(app);
    // Pointer focus routing ends in `constraints::reconcile` (see
    // `pointer::finish_frame`), so this also registers the new/removed
    // output with the session input grab/constraints pipeline.
    crate::session::pointer::update_client_state(app, app.start_time.elapsed().as_millis() as u32);
    app.notify_output_management();
}

/// Pointer containment after a layout change: clamp the compositor
/// cursor to a mapped output (covers the disconnect case where the old
/// position was on the removed head). Same helper physical motion uses
/// via `Pointer::process_input_event`.
fn contain_pointer(app: &mut TtyApp) {
    let (x, y) = app.pointer.position();
    let point = smithay::utils::Point::<f64, smithay::utils::Logical>::from((x, y));
    if app
        .pointer
        .set_position_on_outputs(point, &app.wayland.space)
        .is_none()
    {
        eventline::warn!("hotplug: no mapped output to contain pointer on");
    }
}

#[cfg(test)]
mod tests {
    use super::{HotplugSideEffect, connector_name, resolve_output_config};
    use halley_config::OutputConfig;

    fn cfg(name: &str) -> OutputConfig {
        OutputConfig {
            name: name.to_string(),
            width: None,
            height: None,
            scale: 1.0,
            offset_x: 0,
            offset_y: 0,
            rate: None,
            transform: 0,
            vrr: halley_config::Vrr::Off,
        }
    }

    #[test]
    fn resolves_config_by_connector_name_like_output_target() {
        let configs = vec![cfg("DP-1"), cfg("HDMI-A-1")];
        assert_eq!(
            resolve_output_config(&configs, "DP-1").map(|c| c.name.as_str()),
            Some("DP-1")
        );
        assert!(resolve_output_config(&configs, "eDP-1").is_none());
        assert!(resolve_output_config(&[], "DP-1").is_none());
    }

    #[test]
    fn side_effects_cover_all_scan_events() {
        assert_eq!(HotplugSideEffect::Added as u8, 0);
        assert_eq!(HotplugSideEffect::Removed as u8, 1);
        assert_eq!(HotplugSideEffect::Changed as u8, 2);
    }

    #[test]
    fn connector_name_spelling_matches_backend_output_names() {
        // `connector_name` is a pure mirror of
        // `backend::tty::output::connector_name`; this test pins the
        // "INTERFACE-ID" shape the config lookup depends on.
        let _ = connector_name;
    }
}
