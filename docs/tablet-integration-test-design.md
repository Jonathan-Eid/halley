# Tablet-v2 integration test design

Status: **partial harness implemented** in `tests/support/tablet_nested.rs` and the nested server test input. The baseline fails at tablet announcement; pressure and surface-focus assertions are present but cannot run past that failure until routing exists. Tip, button, multi-client transitions, and XWayland tablet coverage remain future work. The existing `tablet_v2_is_advertised_to_clients` test checks the global, not input delivery. This design targets the production input handler and real protocol clients without restarting or injecting input into the host desktop.

## Existing seam and important constraint

`src/session/drivers/winit/test_support.rs` runs only in the test server and receives commands over `HALLEY_TEST_WINIT_CONTROL`. Its `TestInput` invokes `session::input::handle` with Smithay `InputEvent`s; tests use `Fixture::with_two_outputs` and `fullscreen_multi_output::Input`. The four tablet associated event types are currently `UnusedEvent`. Merely adding socket commands cannot produce a tablet event.

More importantly, `TestInput::Device` is `WinitVirtualDevice`. At the Smithay revision pinned in `Cargo.lock`, that device reports keyboard, pointer, touch and gesture capabilities, **not TabletTool**. A realistic harness needs a test device implementing Smithay's `Device`, with stable distinct IDs and the TabletTool capability. Replace the test backend's device type and return the appropriate device from *all* existing synthetic event implementations; retain the existing keyboard/pointer capabilities. Emit `DeviceAdded` and `DeviceRemoved` through the production handler, rather than directly registering Smithay tablet objects. Verify that existing nested keyboard/pointer tests still pass after this change.

Implement concrete axis, proximity, tip and button event types satisfying the pinned Smithay traits in `smithay/src/backend/input/tablet.rs`, including `AbsolutePositionEvent`, `TabletToolEvent` and `Event`. Preserve device identity and `TabletToolDescriptor` identity across events; expose distinct tools and devices so lifecycle bugs are observable. Specify explicit timestamps, normalized coordinates, changed-axis flags, pressure, tilt and button state. The test backend is synthetic: this exercises Halley's common input/session path and Wayland delivery, **not** libinput conversion or real hardware.

## Control and client implementation

1. Extend only `cfg(test)`'s datagram command handler with device add/remove and tool proximity/axis/tip/button commands. Make each command carry device/tool IDs and all relevant values. Reply only after the production handler returns; reject invalid input rather than silently converting it. Do not call tablet seat methods or select a focused surface directly from the control socket.
2. Add corresponding methods to the test `Input` wrapper. Create an ignored `tests/support/tablet_nested.rs` module included by `tests/xwayland_disconnect.rs`.
3. Bind `zwp_tablet_manager_v2`, request a tablet seat for the fixture's `wl_seat`, and implement client Dispatch handlers for tablet-seat, tablet and tool objects. Record ordered events and their object identities. Use `native_window` and existing fixture placement/state helpers to locate mapped client content; use `EventQueue::roundtrip` and bounded `wait_for` loops, not sleeps, to observe protocol progress. The datagram reply proves handler completion, not that the client dispatched the resulting events.
4. Capture event streams per client; assert local coordinates against the actual mapped surface geometry, allowing Wayland fixed-point rounding. Avoid assuming global and surface coordinates coincide, especially with decorations, output offsets, scaling and camera transforms.
5. Run this harness against the baseline first. The first delivery assertion should fail because baseline `session::input::handle` has no tablet dispatch, despite the advertised global; then apply the implementation and rerun. A compile failure, missing desktop, absent protocol object or fixture timeout is not a valid red result.

## Minimum behavioral cases

- Add a tablet after the client binds: one tablet and tool announcement with stable identities. Proximity in on a mapped native surface must produce proximity-in on that surface, local motion and frame. An axis change must deliver the changed pressure/tilt and frame; tip down/up and tool button press/release must be ordered and paired. Proximity out must stop delivery. Check serials are usable/nonzero where the protocol supplies them and frames delimit tool updates.
- Enter a second client: old client receives proximity-out before the new client receives proximity-in, and neither gets the other's tip/button/axis events. Test both an uncovered surface and a surface with nonzero origin; include transformed/scaled output if supported by the harness.
- Remove an in-proximity device and destroy its focused surface while active in separate cases: no crash, stale events or stuck tip/button state; then add/re-enter and verify clean subsequent delivery. Two tools and two devices must not alias identity or remove one another.
- Include a client which does not bind tablet-v2 and explicitly test the intended pointer fallback behavior. Also check there is no duplicate pointer click when the tablet protocol client consumes the same tip. These assertions must follow the implementation's specified policy, not assume the protocol mandates a particular compositor fallback.
- Exercise compositor-owned regions and active modal/session-lock states: input must follow the selected policy and must not leak to an underlying client. Add tests for focus transitions when a surface is unmapped or a popup changes the hit target.

## XWayland case

Use the same fixture's owned `fixture.display`, never ambient `DISPLAY`, and a real mapped X11 client. Select XI2 events using an X11 library with XI2 support; record the X server's device classes and delivered proximity, motion, pressure and button events. First establish whether this XWayland version advertises a tablet-derived XI2 device after the Wayland device announcement. If it does, assert focused-window delivery and axis/button values; if it does not, explicitly report that capability limitation and test the compositor's documented pointer fallback separately. Core X11 pointer events alone do not prove tablet-v2 or pressure delivery. Do not replace missing XI2 evidence with a passing core-pointer assertion. The precise axis mapping/device enumeration must be discovered from the disposable server, not hardcoded from host XInput IDs.

## Execution and isolation

Build and run the existing server and targeted tests as documented in `docs/testing.md`:

```sh
cargo test --locked --test nested_fullscreen_server --no-run
# Use the executable path reported by Cargo, not an installed Halley binary.
HALLEY_TEST_WINIT_SERVER=/absolute/path/to/reported/executable \
  cargo test --locked --test xwayland_disconnect tablet_ -- --ignored --test-threads=1
```

A working Wayland host, EGL and XWayland are required. The existing fixture creates a private runtime, starts an owned nested compositor and XWayland, and removes only its own processes; see `Fixture::drop`. The nested window is visible on the host, but synthetic events go to the private test socket, not host input devices. Do not use `uinput`, change host configuration, or restart the desktop. On failure preserve fixture diagnostics and report whether the failure was setup, unsupported XWayland capability, or a real protocol assertion. After the targeted run, execute existing nested keyboard/pointer regressions and ordinary workspace tests.


The current seam implements device add/remove and normalized proximity/pressure axis events only. The tablet test is ignored and deliberately red on the baseline. It requires a live host Wayland display, XWayland and the freshly built test server. The test uses the actual primary-output geometry reported by the control socket; pressure/focus assertions require validation against the eventual routing implementation.
