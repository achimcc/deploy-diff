//! The comparison itself: what the old generation had that the new one lacks.

use std::fmt;

use unit_lint::unit::Unit;

use crate::live::Live;
use crate::snapshot::{Input, Snapshot};

#[derive(Debug, PartialEq, Eq)]
pub enum Loss {
    /// A guest the new generation no longer declares. Its units are not
    /// listed one by one — they go with it.
    Guest { name: String },
    /// A unit of the host or of a remaining guest. `unit` is what runs: an
    /// instance when its template disappears.
    Unit { system: String, unit: String },
    /// A flake input that goes back in time — a secret, a pinned tool.
    Input {
        name: String,
        old: Input,
        new: Input,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Note {
    /// No input record in the old (`old = true`) or new generation.
    InputsUnknown { old: bool },
    /// An input the new generation no longer has.
    InputGone { name: String },
    /// A declared guest that does not run; its removal stops nothing.
    GuestNotRunning { name: String },
    /// A running guest whose systemd did not answer: its declared units were
    /// compared instead of its active ones.
    LiveUnreadable { system: String, error: String },
    /// `/run/current-system` is not the profile: a `deploy test` left a
    /// configuration that is in no generation.
    TestResidue { current: String, profile: String },
}

#[derive(Debug, Default)]
pub struct Diff {
    pub losses: Vec<Loss>,
    pub notes: Vec<Note>,
}

pub fn compare(old: &Snapshot, new: &Snapshot, live: Option<&Live>) -> Diff {
    let mut d = Diff::default();

    for guest in old.guests() {
        if new.systems.contains_key(guest) {
            continue;
        }
        match live {
            Some(l) if !l.running(guest) => d.notes.push(Note::GuestNotRunning {
                name: guest.to_owned(),
            }),
            _ => d.losses.push(Loss::Guest {
                name: guest.to_owned(),
            }),
        }
    }

    for (name, sys) in &old.systems {
        // A removed guest is reported once, above.
        let Some(new_sys) = new.systems.get(name) else {
            continue;
        };
        // A guest whose systemd did not answer falls back to its declaration.
        let active = live
            .filter(|l| !l.unreadable.contains_key(name))
            .map(|l| l.active.get(name));
        for unit in &sys.units {
            if new_sys.has(unit) {
                continue;
            }
            match active {
                // Without live state: everything the old generation declared.
                None => d.losses.push(Loss::Unit {
                    system: name.clone(),
                    unit: unit.clone(),
                }),
                // A guest that does not run has no active units to lose.
                Some(None) => {}
                Some(Some(running)) => {
                    for r in running {
                        let matches = r == unit || Unit::template_name(r).as_ref() == Some(unit);
                        if matches && !new_sys.has(r) {
                            d.losses.push(Loss::Unit {
                                system: name.clone(),
                                unit: r.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    match (&old.inputs, &new.inputs) {
        (Some(o), Some(n)) => {
            for (name, oi) in o {
                match n.get(name) {
                    Some(ni) if ni.last_modified < oi.last_modified => d.losses.push(Loss::Input {
                        name: name.clone(),
                        old: oi.clone(),
                        new: ni.clone(),
                    }),
                    Some(_) => {}
                    None => d.notes.push(Note::InputGone { name: name.clone() }),
                }
            }
        }
        (None, _) => d.notes.push(Note::InputsUnknown { old: true }),
        (_, None) => d.notes.push(Note::InputsUnknown { old: false }),
    }

    for (system, error) in live.iter().flat_map(|l| &l.unreadable) {
        d.notes.push(Note::LiveUnreadable {
            system: system.clone(),
            error: error.clone(),
        });
    }
    if let Some((c, p)) = live.and_then(|l| l.test_residue.as_ref()) {
        d.notes.push(Note::TestResidue {
            current: c.display().to_string(),
            profile: p.display().to_string(),
        });
    }
    d
}

fn short(rev: &Option<String>) -> &str {
    rev.as_deref().map_or("?", |r| &r[..r.len().min(8)])
}

fn date(ts: i64) -> String {
    // Days since the epoch to a civil date (Howard Hinnant's algorithm), so
    // the report needs no time crate.
    let z = ts.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

impl fmt::Display for Loss {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Loss::Guest { name } => write!(f, "guest {name} would be removed"),
            Loss::Unit { system, unit } => write!(f, "{system}  {unit} would stop"),
            Loss::Input { name, old, new } => write!(
                f,
                "input {name} goes back: {} ({}) -> {} ({})",
                short(&old.rev),
                date(old.last_modified),
                short(&new.rev),
                date(new.last_modified)
            ),
        }
    }
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Note::InputsUnknown { old: true } => {
                write!(f, "the running generation records no inputs — not compared")
            }
            Note::InputsUnknown { old: false } => {
                write!(f, "the new generation records no inputs — not compared")
            }
            Note::InputGone { name } => write!(f, "input {name} is gone from the new generation"),
            Note::GuestNotRunning { name } => {
                write!(f, "guest {name} is removed, but it does not run")
            }
            Note::LiveUnreadable { system, error } => write!(
                f,
                "{system}: systemd did not answer, its declared units were compared ({error})"
            ),
            Note::TestResidue { current, profile } => write!(
                f,
                "/run/current-system is not the profile — a `deploy test` left it\n    current: {current}\n    profile: {profile}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::date;

    #[test]
    fn civil_dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_000_000_000), "2001-09-09");
        assert_eq!(date(951_782_400), "2000-02-29");
    }
}
