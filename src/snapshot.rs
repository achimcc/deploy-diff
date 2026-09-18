//! What a generation declares: its guests, the units of the host and of each
//! guest, and the revisions of the flake inputs it was built from.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;
use unit_lint::unit::{self, Unit};

/// The name the host's own system carries; guests carry their machine name.
pub const HOST: &str = "host";

/// The unit kinds whose loss stops something that runs. Targets, slices and
/// scopes group other units; devices belong to the kernel.
const KINDS: &[&str] = &[
    "service",
    "socket",
    "timer",
    "path",
    "mount",
    "automount",
    "swap",
];

/// Where a generation records its flake inputs. The host's configuration
/// writes it; a generation without it cannot be compared on inputs.
pub const INPUTS_FILE: &str = "etc/flake-inputs.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Input {
    #[serde(default)]
    pub rev: Option<String>,
    #[serde(rename = "lastModified")]
    pub last_modified: i64,
}

#[derive(Debug, Default)]
pub struct System {
    /// Units with a file behind them. A masked unit does not run.
    pub units: BTreeSet<String>,
}

impl System {
    /// A unit by name, or through its template for an instance.
    pub fn has(&self, name: &str) -> bool {
        self.units.contains(name)
            || Unit::template_name(name).is_some_and(|t| self.units.contains(&t))
    }
}

#[derive(Debug, Default)]
pub struct Snapshot {
    /// `HOST` and one entry per guest.
    pub systems: BTreeMap<String, System>,
    /// `None` when the generation has no `INPUTS_FILE`.
    pub inputs: Option<BTreeMap<String, Input>>,
}

impl Snapshot {
    pub fn guests(&self) -> impl Iterator<Item = &str> {
        self.systems
            .keys()
            .map(String::as_str)
            .filter(|s| *s != HOST)
    }

    pub fn load(top: &Path) -> Result<Self, String> {
        let systems = unit::load_toplevel(HOST, top)
            .map_err(|e| format!("{}: {e}", top.display()))?
            .into_iter()
            .map(|s| {
                let units = s
                    .units
                    .values()
                    .filter(|u| u.file.is_some() && !u.masked && KINDS.contains(&u.kind()))
                    .map(|u| u.name.clone())
                    .collect();
                (s.name, System { units })
            })
            .collect();
        let inputs = match fs::read_to_string(top.join(INPUTS_FILE)) {
            Ok(text) => Some(
                serde_json::from_str(&text)
                    .map_err(|e| format!("{}: {e}", top.join(INPUTS_FILE).display()))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", top.join(INPUTS_FILE).display())),
        };
        Ok(Snapshot { systems, inputs })
    }
}
