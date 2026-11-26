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
    let _window = x11_window(&x11, "XInput tablet target");
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
    let _ = monitor.kill();
    let _ = monitor.wait();
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
