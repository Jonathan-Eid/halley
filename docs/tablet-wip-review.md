# Tablet support: outstanding WIP review

Do not merge or deploy this branch until these source-review findings are addressed and tested. These are static-review findings, not verified hardware reproductions.

- Capture overlays currently do not intercept tablet input; pen events can reach an application beneath screenshot UI. Suppress/cancel tablet streams during capture and test this.
- Confirmation modals can consume tip-up/proximity-out without cancelling an existing stroke. Cancel active streams on modal entry and suppress them until fresh proximity-in.
- Proximity-in can be lost when the initial position has no valid route; later events then remain ignored until re-entry. Track physical proximity independent of focused surface, and clear stale focus when a route disappears.
- Tip events include position and axes; apply changed coordinates and pressure before tip down/up in the same frame.
- Tip-down should apply the normal window activation and keyboard-focus policy.
- Tablet mapping currently reuses touchscreen settings and falls back to the primary output. Provide tablet-specific mapping and verify integrated pen mapping to the appropriate laptop display, including transforms and multi-output cases.
- Pressure axes must not go to an old focused surface when motion changes focus; test ordering and client transitions.
- Lock handling cancels active tools but does not provide lock-surface pen input. Verify isolation across lock/unlock; if lock-screen pen support is intended, route exclusively to lock surfaces.
- Exercise device removal, surface destruction, grab semantics, tool cursor handling, and DPMS wake.
- Run nested tablet-v2 client integration tests and XWayland/Krita pressure tests; unit tests and a protocol global alone do not prove end-to-end behavior.
