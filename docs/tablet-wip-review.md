# Tablet support: WIP review

Nested tablet-v2 protocol tests cover the following with synthetic input; they do not replace physical-device testing:

- Capture and session-lock entry cancel an active stream and isolate ordinary clients. Capture exit requires a fresh proximity-in; unlock recovery remains untested.
- Proximity-in without output geometry is retained. Later motion can acquire focus; loss of geometry or a route clears surface focus. Geometry recovery and stale-tip isolation are covered by a nested test.
- Tip down and up send changed coordinates and pressure before the tip event. Tip down selects the target output and activates the target window; grab semantics and layer focus remain untested.
- Exact-name tablet device mapping overrides the class mapping. A two-output nested test exercises an override targeting a 180-degree transformed output, including client-relative coordinates and window activation. Other transforms, unavailable output fallback, hotplug, and physical eDP hardware remain unverified end-to-end.
- Pressure is routed to the new surface after focus changes and is not sent to a surface after route loss.

Outstanding work:

- Test confirmation-modal cancellation and isolation; verify lock/unlock recovery. Lock-surface pen input is not implemented.
- Exercise device removal, surface destruction, grab semantics, tool cursor handling, and DPMS wake.
- Run physical tablet and XWayland/Krita pressure tests.
