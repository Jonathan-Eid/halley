//! Manual session snapshots for flat Field windows.
//!
//! This is deliberately narrow: a snapshot records the application id, title,
//! output, and geometry of every ordinary expanded Field window. It does not
//! record clusters, collapsed nodes, camera state, focus history, or decay
//! timers, and nothing is ever written automatically.
//!
//! Loading cannot resurrect live Wayland clients, so it relaunches each
//! recorded application and places the first matching window that appears at
//! the saved geometry. Applications that restore their own windows, refuse a
//! second launch, or expose no usable `app_id` are matched on a best-effort
//! basis.

use std::collections::{HashSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use smithay::utils::{Logical, Point};

use super::{Session, SessionDriver};
use crate::nodes::ipc::{place_node, set_node_size};
use halley_core::field::NodeId;

const FILE_NAME: &str = "session-restore.postcard";
const FORMAT_VERSION: u32 = 1;
/// How long to wait for a relaunched application to present a window.
const MATCH_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    saved_at_unix: u64,
    windows: Vec<SavedWindow>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SavedWindow {
    app_id: String,
    title: String,
    output: String,
    /// Client top-left relative to the output's Field origin.
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    focused: bool,
}

/// Data-only state parked on `LaunchState`; all behavior lives in this module.
#[derive(Default)]
pub(crate) struct RestoreState {
    plan: Option<RestorePlan>,
}

struct PendingLaunch {
    record: SavedWindow,
    command: String,
}

struct RestorePlan {
    pending: VecDeque<PendingLaunch>,
    active: Option<ActiveLaunch>,
    claimed: HashSet<u64>,
}

struct ActiveLaunch {
    saved: SavedWindow,
    app_id: String,
    preexisting: HashSet<u64>,
    deadline: Instant,
}

impl RestorePlan {
    fn is_done(&self) -> bool {
        self.pending.is_empty() && self.active.is_none()
    }

    fn step<D: SessionDriver>(&mut self, session: &mut Session<D>) {
        let now = Instant::now();
        if let Some(active) = &self.active {
            if let Some(id) = find_candidate(session, &active.app_id, &active.preexisting, &self.claimed)
            {
                let saved = active.saved.clone();
                self.claimed.insert(id.as_u64());
                self.active = None;
                place_window(session, id, &saved);
            } else if now >= active.deadline {
                eventline::warn!(
                    "session restore: no window appeared for {:?} within {}s",
                    active.app_id,
                    MATCH_TIMEOUT.as_secs()
                );
                self.active = None;
            }
            return;
        }

        while let Some(PendingLaunch { record: saved, command }) = self.pending.pop_front() {
            if saved.app_id.is_empty() {
                continue;
            }
            if let Some(id) = find_candidate(session, &saved.app_id, &HashSet::new(), &self.claimed) {
                self.claimed.insert(id.as_u64());
                place_window(session, id, &saved);
                continue;
            }
            let Some(display) = session.wayland_display.clone() else {
                eventline::warn!("session restore: no Wayland display name; aborting");
                return;
            };
            let preexisting = session.nodes.records().map(|record| record.id.as_u64()).collect();
            let x11_display = session.xwayland.display_name();
            let app_id = saved.app_id.clone();
            eventline::info!(
                "session restore: launching {:?} for app id {:?}",
                command,
                app_id
            );
            super::spawn::spawn_detached(
                &command,
                &display,
                x11_display.as_deref(),
                session.cursor.size(),
                session.launch.environment(),
            );
            self.active = Some(ActiveLaunch {
                saved,
                app_id,
                preexisting,
                deadline: now + MATCH_TIMEOUT,
            });
            return;
        }
    }
}

fn find_candidate<D: SessionDriver>(
    session: &Session<D>,
    app_id: &str,
    preexisting: &HashSet<u64>,
    claimed: &HashSet<u64>,
) -> Option<NodeId> {
    session
        .nodes
        .records()
        .filter(|record| record.attached && !record.collapsed)
        .filter(|record| !preexisting.contains(&record.id.as_u64()))
        .filter(|record| !claimed.contains(&record.id.as_u64()))
        .find(|record| {
            record
                .app_id
                .as_deref()
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(app_id))
        })
        .map(|record| record.id)
}

fn place_window<D: SessionDriver>(session: &mut Session<D>, id: NodeId, saved: &SavedWindow) {
    set_node_size(session, id, saved.width, saved.height);
    let Some(record) = session.nodes.record(id).cloned() else {
        return;
    };
    let output = if saved.output.is_empty() {
        record.output.clone()
    } else {
        saved.output.clone()
    };
    let origin = output_origin(session, &output).or_else(|| output_origin(session, &record.output));
    let Some(origin) = origin else {
        eventline::warn!("session restore: output {:?} is not present; leaving node in place", output);
        return;
    };
    let destination = Point::from((origin.x + saved.x, origin.y + saved.y));
    if !place_node(session, id, (0, 0), false, Some(destination)) {
        eventline::warn!(
            "session restore: could not place node {} at {:?}",
            id.as_u64(),
            destination
        );
    }
    eventline::info!(
        "session restore: placed node {} ({:?}) at {},{}",
        id.as_u64(),
        saved.title,
        saved.x,
        saved.y
    );
}

fn output_origin<D: SessionDriver>(session: &Session<D>, name: &str) -> Option<Point<i32, Logical>> {
    session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == name)
        .and_then(|output| session.wayland.space.output_geometry(output))
        .map(|geometry| geometry.loc)
}

/// Writes a manual snapshot. Returns the path and the number of windows saved.
pub(crate) fn save<D: SessionDriver>(session: &mut Session<D>) -> io::Result<(PathBuf, usize)> {
    session.nodes.sync_from_space(&session.wayland.space);
    let mut windows = Vec::new();
    for record in session.nodes.records() {
        if !record.attached || record.collapsed {
            continue;
        }
        if record.app_id.is_none() {
            continue;
        }
        if crate::xwayland::is_override_redirect(&record.window) {
            continue;
        }
        if session.fullscreen.is_fullscreen_or_pending(&record.surface)
            || session.maximize.contains(&record.surface)
        {
            continue;
        }
        let output = record.output.clone();
        let Some(origin) = output_origin(session, &output) else {
            continue;
        };
        windows.push(SavedWindow {
            app_id: record.app_id.clone().unwrap_or_default(),
            title: record.title.clone(),
            output,
            x: record.geometry.loc.x - origin.x,
            y: record.geometry.loc.y - origin.y,
            width: record.geometry.size.w,
            height: record.geometry.size.h,
            focused: session.nodes.focused() == Some(record.id),
        });
    }
    windows.sort_by(|a, b| {
        (a.output.as_str(), a.x, a.y, a.title.as_str())
            .cmp(&(b.output.as_str(), b.x, b.y, b.title.as_str()))
    });

    let file = SessionFile {
        version: FORMAT_VERSION,
        saved_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0),
        windows: windows.clone(),
    };
    let path = file_path().ok_or_else(|| io::Error::other("no state directory"))?;
    let bytes = postcard::to_allocvec(&file).map_err(io::Error::other)?;
    write_atomic(&path, &bytes)?;
    eventline::info!(
        "session restore: saved {} window(s) to {}",
        windows.len(),
        path.display()
    );
    Ok((path, windows.len()))
}

/// Loads the snapshot into a restore plan. The event loop advances it.
pub(crate) fn load<D: SessionDriver>(session: &mut Session<D>) -> io::Result<usize> {
    let path = file_path().ok_or_else(|| io::Error::other("no state directory"))?;
    let bytes = fs::read(&path)?;
    let file: SessionFile = postcard::from_bytes(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    if file.version != FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "session file version {} is not supported (expected {FORMAT_VERSION})",
                file.version
            ),
        ));
    }
    let count = file.windows.len();
    let mut skipped = Vec::new();
    let mut pending = VecDeque::new();
    for window in file.windows {
        match resolve_command(&window.app_id) {
            Some(command) => pending.push_back(PendingLaunch { record: window, command }),
            None => skipped.push(window),
        }
    }
    for window in skipped {
        eventline::warn!(
            "session restore: no launch command for app id {:?}; skipping",
            window.app_id
        );
    }
    session.launch.restore.plan = Some(RestorePlan {
        pending,
        active: None,
        claimed: HashSet::new(),
    });
    eventline::info!(
        "session restore: loaded {} window(s) from {}",
        count,
        path.display()
    );
    Ok(count)
}

/// Advances an active restore plan. Safe to call every event-loop iteration.
pub(crate) fn tick<D: SessionDriver>(session: &mut Session<D>) {
    let Some(mut plan) = session.launch.restore.plan.take() else {
        return;
    };
    plan.step(session);
    if plan.is_done() {
        eventline::info!("session restore: finished");
    } else {
        session.launch.restore.plan = Some(plan);
    }
}

fn file_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(base.join("halley").join(FILE_NAME))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(io::Error::other("state path has no parent"));
    };
    fs::create_dir_all(parent)?;
    let mut permissions = fs::metadata(parent)?.permissions();
    permissions.set_mode(0o700);
    let _ = fs::set_permissions(parent, permissions);

    let temporary = parent.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
    fs::write(&temporary, bytes)?;
    let mut file_permissions = fs::metadata(&temporary)?.permissions();
    file_permissions.set_mode(0o600);
    fs::set_permissions(&temporary, file_permissions)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

/// Resolves an application id to a command line using desktop entries, then a
/// PATH lookup as a fallback.
fn resolve_command(app_id: &str) -> Option<String> {
    if let Some(command) = desktop_command(app_id) {
        return Some(command);
    }
    if which(app_id) {
        return Some(app_id.to_string());
    }
    None
}

fn desktop_command(app_id: &str) -> Option<String> {
    let mut directories = Vec::new();
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        directories.push(PathBuf::from(data_home).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME") {
        directories.push(PathBuf::from(home).join(".local/share/applications"));
    }
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    for directory in data_dirs.split(':').filter(|entry| !entry.is_empty()) {
        directories.push(PathBuf::from(directory).join("applications"));
    }

    for directory in directories {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension() != Some(OsStr::new("desktop")) {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Some((exec, startup_class, hidden)) = parse_desktop_entry(&text) else {
                continue;
            };
            if hidden {
                continue;
            }
            let stem_matches = stem.eq_ignore_ascii_case(app_id);
            let class_matches = startup_class
                .as_deref()
                .is_some_and(|class| class.eq_ignore_ascii_case(app_id));
            if stem_matches || class_matches {
                return clean_exec(&exec);
            }
        }
    }
    None
}

fn parse_desktop_entry(text: &str) -> Option<(String, Option<String>, bool)> {
    let mut in_entry = false;
    let mut exec = None;
    let mut startup_class = None;
    let mut hidden = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Exec" => exec = Some(value.trim().to_string()),
            "StartupWMClass" => startup_class = Some(value.trim().to_string()),
            "NoDisplay" | "Hidden" => hidden |= value.trim().eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    exec.map(|exec| (exec, startup_class, hidden))
}

fn clean_exec(exec: &str) -> Option<String> {
    let flags = ["--gtk-single-instance", "--new-window", "--window-new"];
    let mut parts = Vec::new();
    for token in exec.split_whitespace() {
        if token.starts_with('%') && token.len() == 2 {
            continue;
        }
        if flags.contains(&token) {
            continue;
        }
        parts.push(token);
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn which(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
}
