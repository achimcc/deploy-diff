//! What runs right now: the machines, the active units of the host and of
//! each running guest, and whether `/run/current-system` is the profile.
//!
//! Without this, every unit a generation declares counts — including the ones
//! that never run (disabled, conditioned away, a oneshot long finished). With
//! it, a loss is something that is running and would stop.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
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

/// Asks systemd on this host. Needs root for `systemctl -M`.
pub fn collect() -> Result<Live, String> {
    let mut live = Live::default();
    live.active
        .insert(HOST.to_owned(), parse_units(&run("systemctl", LIST)?)?);
    for m in parse_machines(&run("machinectl", &["list", "-o", "json"])?)? {
        let mut args = vec!["-M", m.as_str()];
        args.extend_from_slice(LIST);
        match run("systemctl", &args).and_then(|j| parse_units(&j)) {
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
