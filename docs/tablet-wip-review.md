# Tablet support: WIP review

This review describes the integrated tree at `2b9ec6f7`. The nested tests below use synthetic input through the test backend and real protocol clients, not libinput or physical hardware. They are ignored by default and require a Wayland host, XWayland and a freshly built test server; see `docs/tablet-integration-test-design.md` for the invocation. Test coverage described here is not a claim that those tests passed in this review: no Wayland display was available to run them.

## Checked in this review

`cargo test --locked --bin halley tablet_lifecycle -- --test-threads=1` passed six unit tests. These cover lifecycle state and include source-level checks for capture, confirmation, cluster-composer isolation and output wake; source checks are not end-to-end behavioral tests.

## Synthetic nested test coverage

- Tablet-v2 announcement, pressure, surface focus, tip event ordering, focused tool cursor selection and absence of duplicate pointer clicks for a tablet-aware client. The two-output test checks an exact-name device override on a 180-degree transformed output, client coordinates and window activation.
- Capture and session-lock entry cancel active streams and isolate ordinary clients. Capture exit requires fresh proximity. A separate test checks that session lock releases a held pointer-fallback button.
- Missing output geometry, subsequent focus acquisition, route loss and stale-tip isolation; pressure follows surface focus and stops after route loss.
- Pointer-only clients receive tip fallback; pointer lock blocks fallback motion, confinement clamps it, and titlebar dragging uses compositor grabs. Separate tests cover physical tablet movement over a titlebar while the pointer is locked or confined (still synthetic events).
- An XWayland XI2 test checks a mapped X11 window for stylus tip press/release and pressure values, alongside device enumeration and raw XI2 observations. This is more than a core-pointer test, but remains synthetic and depends on the installed XWayland's capabilities.

## Outstanding

- Pen input to lock surfaces is not implemented. Test unlock recovery and lock-surface behavior when implemented; isolation of ordinary clients is not proof that a lock screen can receive pen input.
- Exercise confirmation-modal and cluster-composer cancellation/isolation end-to-end, device removal, surface destruction, broader grab and layer-focus behavior, cursor lifecycle and actual DPMS wake. Unit/source checks do not establish these end-to-end.
- Verify other output transforms, unavailable-output fallback, hotplug and physical eDP mapping. Run physical tablet tests, including libinput behavior and pressure, and test real XWayland applications such as Krita; synthetic XI2 coverage does not establish hardware or application compatibility.
