//! XInput2 tablet compatibility through the real nested XWayland server.
use super::*;

#[test]
#[ignore = "requires Wayland, XWayland, xinput, stdbuf and HALLEY_TEST_WINIT_SERVER"]
fn xwayland_tablet_tool_and_pressure() {
    let mut fixture = Fixture::with_two_outputs(
        "animations:\n enabled false\nend\ninput:\n tablet:\n  map-to-output \"winit-2\"\n end\nend\n",
    );
    let input = fullscreen_multi_output::Input::new(&fixture);
    let (x11, _) = x11rb::connect(Some(&fixture.display)).unwrap();
    let target = x11_window(&x11, "XInput tablet target");
    let node = fixture.node("XInput tablet target");
    fullscreen_multi_output::place(&mut fixture, node.id, true);
    let state = input.state();
    let window = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == node.id)
        .unwrap();
    let rect = window["client_geometry"].as_array().unwrap();
    let output = state["outputs_geometry"]["winit-2"].as_array().unwrap();
    let nx = (rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() * 0.5
        - output[0].as_f64().unwrap())
        / output[2].as_f64().unwrap();
    let ny = (rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() * 0.5
        - output[1].as_f64().unwrap())
        / output[3].as_f64().unwrap();
    input.command("tablet-add 1");
    input.command(&format!("tablet-proximity 1 42 {nx} {ny} 0.5 in"));
    let listing = wait_for("XInput tablet tool", || {
        let out = Command::new("xinput")
            .env("DISPLAY", &fixture.display)
            .args(["--list", "--short"])
            .output()
            .unwrap();
        let s = String::from_utf8_lossy(&out.stdout).into_owned();
        eprintln!("XInput devices: {s}");
        s.lines()
            .any(|line| line.to_lowercase().contains("stylus"))
            .then_some(s)
    });
    for name in [
        "xwayland-tablet stylus",
        "xwayland-tablet eraser",
        "xwayland-tablet cursor",
    ] {
        assert!(listing.contains(name), "missing {name}: {listing}");
    }
    let line = listing
        .lines()
        .find(|line| line.contains("xwayland-tablet stylus"))
        .unwrap();
    let id = line
        .split("id=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    let stylus: u16 = id.parse().unwrap();
    use x11rb::protocol::xinput::{ConnectionExt as _, EventMask, XIEventMask};
    x11.xinput_xi_query_version(2, 0).unwrap().reply().unwrap();
    x11.xinput_xi_select_events(
        target,
        &[EventMask {
            deviceid: stylus,
            mask: vec![
                XIEventMask::BUTTON_PRESS | XIEventMask::BUTTON_RELEASE | XIEventMask::MOTION,
            ],
        }],
    )
    .unwrap()
    .check()
    .unwrap();
    x11.flush().unwrap();
    let log_path = fixture.path.join("xi-events.log");
    let log = File::create(&log_path).unwrap();
    let mut monitor = Command::new("stdbuf")
        .args(["-oL", "xinput", "test-xi2", "--root", id])
        .env("DISPLAY", &fixture.display)
        .stdout(Stdio::from(log))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(500));
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.75 down"));
    input.command(&format!("tablet-axis 1 42 {nx} {ny} 0.5 in"));
    input.command(&format!("tablet-tip 1 42 {nx} {ny} 0.0 up"));
    thread::sleep(Duration::from_millis(500));
    let mut delivered = Vec::new();
    while let Some(event) = x11.poll_for_event().unwrap() {
        use x11rb::protocol::Event;
        let (kind, event) = match event {
            Event::XinputButtonPress(e) => ("press", e),
            Event::XinputButtonRelease(e) => ("release", e),
            Event::XinputMotion(e) => ("motion", e),
            _ => continue,
        };
        if event.event != target || event.sourceid != stylus {
            continue;
        }
        let pressure = if event.valuator_mask.first().copied().unwrap_or(0) & 4 != 0 {
            let index = (event.valuator_mask[0] & 3).count_ones() as usize;
            Some(event.axisvalues[index].integral)
        } else {
            None
        };
        delivered.push((kind, event.detail, pressure));
    }
    let _ = monitor.kill();
    let _ = monitor.wait();
    eprintln!("Window-targeted stylus events: {delivered:?}");
    assert!(
        delivered.iter().any(|(k, d, _)| *k == "press" && *d == 1),
        "missing targeted press: {delivered:?}"
    );
    assert!(
        delivered.iter().any(|(k, d, _)| *k == "release" && *d == 1),
        "missing targeted release: {delivered:?}"
    );
    assert!(
        delivered
            .iter()
            .any(|(_, _, p)| matches!(p, Some(49151 | 49152))),
        "missing targeted 0.75 pressure: {delivered:?}"
    );
    assert!(
        delivered
            .iter()
            .any(|(_, _, p)| matches!(p, Some(32767 | 32768))),
        "missing targeted 0.5 pressure: {delivered:?}"
    );
    let events = fs::read_to_string(log_path).unwrap();
    eprintln!("XInput events:\n{events}");
    assert!(
        events.contains("Label: Abs Pressure"),
        "missing pressure valuator: {events}"
    );
    assert!(
        events.contains("RawButtonPress") && events.contains("RawButtonRelease"),
        "missing tip transitions: {events}"
    );
    assert!(
        events
            .lines()
            .any(|line| line.trim_start().starts_with("2: 49151.")
                || line.trim_start().starts_with("2: 49152.")),
        "no 0.75 pressure on valuator 2: {events}"
    );
    assert!(
        events
            .lines()
            .any(|line| line.trim_start().starts_with("2: 32767.")
                || line.trim_start().starts_with("2: 32768.")),
        "no 0.5 pressure on valuator 2: {events}"
    );
}
