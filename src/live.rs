//! What runs right now: the machines, the active units of the host and of
//! each running guest, and whether `/run/current-system` is the profile.
//!
//! Without this, every unit a generation declares counts — including the ones
//! that never run (disabled, conditioned away, a oneshot long finished). With
//! it, a loss is something that is running and would stop.
//!
//! A guest's own systemd is not the only witness (audit 3 of the homeserver,
//! B109): a guest that reports no units would hide every loss in itself, and
//! the STOP would not come. What the host sees of the guest — the populated
//! unit cgroups below its payload, kernel files read on the host — is added
//! to what the guest reports. That raises the bar, it is no barrier: root in
//! a guest owns its cgroup subtree and can still move a unit's processes
//! elsewhere, or report instances that do not exist. deploy-diff guards
//! against mistakes, not against a guest that was taken over.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::snapshot::HOST;

#[derive(Debug, Default)]
pub struct Live {
    /// The running containers.
    pub machines: BTreeSet<String>,
    /// Active units per system, keyed like `Snapshot::systems`.
    pub active: BTreeMap<String, BTreeSet<String>>,
    /// Running guests whose systemd did not answer — a guest that is booting
    /// or shutting down. Their declared units count instead.
    pub unreadable: BTreeMap<String, String>,
    /// Set when `/run/current-system` and the system profile differ — what a
    /// `switch-to-configuration test` leaves behind.
    pub test_residue: Option<(PathBuf, PathBuf)>,
}

impl Live {
    pub fn running(&self, guest: &str) -> bool {
        self.machines.contains(guest)
    }
}

#[derive(Deserialize)]
struct UnitRow {
    unit: String,
    active: String,
}

#[derive(Deserialize)]
struct MachineRow {
    machine: String,
    class: String,
}

/// `systemctl list-units --all -o json`: the units that run or are about to.
/// `activating` is a oneshot at work, `reloading` still serves.
///
/// A name systemd would never accept is dropped (audit 3 of the homeserver,
/// B81): only a guest's own systemd can report one, it can match no declared
/// unit, and printed raw it would carry escapes to the operator's terminal.
pub fn parse_units(json: &str) -> Result<BTreeSet<String>, String> {
    let rows: Vec<UnitRow> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .filter(|r| {
            matches!(
                r.active.as_str(),
                "active" | "activating" | "reloading" | "refreshing"
            )
        })
        .filter(|r| crate::text::valid_unit_name(&r.unit))
        .map(|r| r.unit)
        .collect())
}

/// `machinectl list -o json`: the running containers. VMs are no guests here.
pub fn parse_machines(json: &str) -> Result<Vec<String>, String> {
    let rows: Vec<MachineRow> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .filter(|r| r.class == "container")
        .map(|r| r.machine)
        .collect())
}

/// The programs deploy-diff runs: the running system's own, never found
/// through PATH (B110). On a NixOS target that is the systemd that answers —
/// deploy-diff itself comes from the generation about to be activated.
pub const SYSTEMCTL: &str = "/run/current-system/sw/bin/systemctl";
pub const MACHINECTL: &str = "/run/current-system/sw/bin/machinectl";
pub const PROGRAMS: &[&str] = &[SYSTEMCTL, MACHINECTL];

/// The host's cgroup v2 hierarchy.
pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Slices nest; a guest can nest them without end. Deeper than this no
/// systemd puts a unit.
const MAX_SLICE_DEPTH: usize = 8;

/// The guest's payload cgroup (`/machine.slice/…/payload`), from the
/// kernel's `/proc/<leader>/cgroup` read on the host. The leader is the
/// guest's systemd, which sits in `<payload>/init.scope` once it has booted;
/// anywhere else the view is not trusted.
pub fn guest_payload(leader_cgroup: &str) -> Result<String, String> {
    let path = leader_cgroup
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .ok_or("the leader is in no cgroup v2")?
        .trim();
    match path.strip_suffix("/init.scope") {
        Some(p) if p.starts_with('/') && !p.split('/').any(|c| c == "..") => Ok(p.to_owned()),
        _ => Err(format!(
            "the leader is not in init.scope: {}",
            crate::text::visible(path)
        )),
    }
}

/// The units the host sees running in a guest: every unit cgroup below the
/// payload whose `cgroup.events` says `populated 1` — the kernel's word,
/// read on the host. Units sit in slices, so only slices are descended;
/// scopes (the guest's own `init.scope`) are no units a generation declares.
/// `root` is `CGROUP_ROOT`, `payload` what `guest_payload` returned.
pub fn host_view(root: &Path, payload: &str) -> Result<BTreeSet<String>, String> {
    let dir = root.join(payload.trim_start_matches('/'));
    let mut seen = BTreeSet::new();
    walk(&dir, 0, &mut seen).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(seen)
}

fn walk(dir: &Path, depth: usize, seen: &mut BTreeSet<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        // cgroupfs has no symlinks; a test tree could. Neither is followed.
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !crate::text::valid_unit_name(&name) || name.starts_with('.') {
            continue;
        }
        if name.ends_with(".slice") {
            if depth < MAX_SLICE_DEPTH {
                walk(&entry.path(), depth + 1, seen)?;
            }
        } else if !name.ends_with(".scope") && populated(&entry.path()) {
            seen.insert(name);
        }
    }
    Ok(())
}

/// `populated 1` in the cgroup's `cgroup.events`: a process in it or below.
fn populated(cgroup: &Path) -> bool {
    let mut s = String::new();
    std::fs::File::open(cgroup.join("cgroup.events"))
        .and_then(|f| f.take(4096).read_to_string(&mut s))
        .is_ok()
        && s.lines().any(|l| l.trim() == "populated 1")
}

/// A guest's active units: what its systemd reports AND what the host sees
/// running in it. Either witness failing makes the guest unreadable — its
/// declared units then count, which a guest cannot shrink.
pub fn guest_active(
    reported: Result<BTreeSet<String>, String>,
    seen: Result<BTreeSet<String>, String>,
) -> Result<BTreeSet<String>, String> {
    let mut units = reported?;
    units.extend(seen.map_err(|e| format!("host cgroup view: {e}"))?);
    Ok(units)
}

/// `machinectl show -p Leader`: the guest's init as a host pid.
fn leader(machine: &str) -> Result<u32, String> {
    run(MACHINECTL, &["show", machine, "-p", "Leader"])?
        .lines()
        .find_map(|l| l.strip_prefix("Leader="))
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| format!("{MACHINECTL}: no leader for {machine}"))
}

/// What the host sees running in `machine`.
fn seen_by_host(machine: &str) -> Result<BTreeSet<String>, String> {
    let pid = leader(machine)?;
    let mut cg = String::new();
    std::fs::File::open(format!("/proc/{pid}/cgroup"))
        .and_then(|f| f.take(1 << 16).read_to_string(&mut cg))
        .map_err(|e| format!("/proc/{pid}/cgroup: {e}"))?;
    host_view(Path::new(CGROUP_ROOT), &guest_payload(&cg)?)
}

fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{cmd} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("{cmd}: {e}"))
}

const LIST: &[&str] = &[
    "list-units",
    "--all",
    "--no-legend",
    "--plain",
    "-o",
    "json",
];

/// Asks systemd on this host and reads the guests' cgroups. Needs root for
/// `systemctl -M`.
pub fn collect() -> Result<Live, String> {
    let mut live = Live::default();
    live.active
        .insert(HOST.to_owned(), parse_units(&run(SYSTEMCTL, LIST)?)?);
    for m in parse_machines(&run(MACHINECTL, &["list", "-o", "json"])?)? {
        let mut args = vec!["-M", m.as_str()];
        args.extend_from_slice(LIST);
        let reported = run(SYSTEMCTL, &args).and_then(|j| parse_units(&j));
        match guest_active(reported, seen_by_host(&m)) {
            Ok(units) => {
                live.active.insert(m.clone(), units);
            }
            Err(e) => {
                live.unreadable.insert(m.clone(), e);
            }
        }
        live.machines.insert(m);
    }
    let current = std::fs::canonicalize("/run/current-system").map_err(|e| e.to_string())?;
    let profile =
        std::fs::canonicalize("/nix/var/nix/profiles/system").map_err(|e| e.to_string())?;
    if current != profile {
        live.test_residue = Some((current, profile));
    }
    Ok(live)
}
