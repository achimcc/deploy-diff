//! The comparison against small toplevels built on disk: the same layout
//! NixOS writes (`etc/systemd/system`, `etc/nixos-containers/*.conf`).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use deploy_diff::diff::{Loss, Note, compare};
use deploy_diff::live::{Live, parse_machines, parse_units};
use deploy_diff::snapshot::{HOST, Snapshot};
use deploy_diff::verdict::{Verdict, decide};

struct Top {
    dir: PathBuf,
}

impl Top {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "deploy-diff-{}-{tag}-{}",
            std::process::id(),
            rand_suffix()
        ));
        fs::create_dir_all(dir.join("etc/systemd/system")).unwrap();
        Top { dir }
    }

    fn unit(self, name: &str) -> Self {
        unit_in(&self.dir, name);
        self
    }

    fn guest(self, name: &str, units: &[&str]) -> Self {
        let g = self.dir.join("guests").join(name);
        fs::create_dir_all(g.join("etc/systemd/system")).unwrap();
        for u in units {
            unit_in(&g, u);
        }
        let confs = self.dir.join("etc/nixos-containers");
        fs::create_dir_all(&confs).unwrap();
        fs::write(
            confs.join(format!("{name}.conf")),
            format!("SYSTEM_PATH={}\n", g.display()),
        )
        .unwrap();
        self
    }

    fn inputs(self, json: &str) -> Self {
        fs::write(self.dir.join("etc/flake-inputs.json"), json).unwrap();
        self
    }

    fn load(&self) -> Snapshot {
        Snapshot::load(&self.dir).unwrap()
    }
}

impl Drop for Top {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

fn unit_in(top: &Path, name: &str) {
    fs::write(
        top.join("etc/systemd/system").join(name),
        "[Service]\nExecStart=/bin/true\n",
    )
    .unwrap();
}

fn live(systems: &[(&str, &[&str])]) -> Live {
    let mut l = Live::default();
    for (s, units) in systems {
        if *s != HOST {
            l.machines.insert((*s).to_owned());
        }
        l.active.insert(
            (*s).to_owned(),
            units
                .iter()
                .map(|u| (*u).to_owned())
                .collect::<BTreeSet<_>>(),
        );
    }
    l
}

fn guest(name: &str) -> Loss {
    Loss::Guest {
        name: name.to_owned(),
    }
}

fn unit(system: &str, unit: &str) -> Loss {
    Loss::Unit {
        system: system.to_owned(),
        unit: unit.to_owned(),
    }
}

#[test]
fn identical_generations_lose_nothing() {
    let a = Top::new("a")
        .unit("sshd.service")
        .guest("req-01", &["seerr.service"]);
    let b = Top::new("b")
        .unit("sshd.service")
        .guest("req-01", &["seerr.service"]);
    let d = compare(&a.load(), &b.load(), None);
    assert!(d.losses.is_empty(), "{:?}", d.losses);
}

#[test]
fn a_removed_guest_is_one_loss_not_one_per_unit() {
    let a = Top::new("a").guest("req-01", &["seerr.service", "caddy.service"]);
    let b = Top::new("b");
    let d = compare(&a.load(), &b.load(), None);
    assert_eq!(d.losses, vec![guest("req-01")]);
}

#[test]
fn a_removed_guest_that_does_not_run_is_a_note() {
    let a = Top::new("a").guest("etv-01", &["ersatztv.service"]);
    let b = Top::new("b");
    let d = compare(&a.load(), &b.load(), Some(&live(&[(HOST, &[])])));
    assert!(d.losses.is_empty());
    assert!(d.notes.contains(&Note::GuestNotRunning {
        name: "etv-01".into()
    }));
}

#[test]
fn without_live_state_every_declared_unit_counts() {
    let a = Top::new("a")
        .unit("lan6-set.timer")
        .unit("lan6-set.service");
    let b = Top::new("b").unit("lan6-set.service");
    let d = compare(&a.load(), &b.load(), None);
    assert_eq!(d.losses, vec![unit(HOST, "lan6-set.timer")]);
}

#[test]
fn with_live_state_only_running_units_count() {
    let a = Top::new("a").unit("running.service").unit("idle.service");
    let b = Top::new("b");
    let l = live(&[(HOST, &["running.service"])]);
    let d = compare(&a.load(), &b.load(), Some(&l));
    assert_eq!(d.losses, vec![unit(HOST, "running.service")]);
}

#[test]
fn targets_and_slices_are_not_losses() {
    let a = Top::new("a").unit("x.target").unit("y.slice");
    let b = Top::new("b");
    let d = compare(&a.load(), &b.load(), None);
    assert!(d.losses.is_empty(), "{:?}", d.losses);
}

#[test]
fn an_instance_is_lost_with_its_template() {
    let a = Top::new("a").unit("mail-bei-fehler@.service");
    let b = Top::new("b");
    let l = live(&[(HOST, &["mail-bei-fehler@lan6-set.service"])]);
    let d = compare(&a.load(), &b.load(), Some(&l));
    assert_eq!(
        d.losses,
        vec![unit(HOST, "mail-bei-fehler@lan6-set.service")]
    );
}

#[test]
fn an_instance_survives_when_the_template_stays() {
    let a = Top::new("a").unit("container@.service");
    let b = Top::new("b").unit("container@.service");
    let l = live(&[(HOST, &["container@req-01.service"])]);
    let d = compare(&a.load(), &b.load(), Some(&l));
    assert!(d.losses.is_empty());
}

#[test]
fn a_unit_in_a_remaining_guest_is_named_with_its_guest() {
    let a = Top::new("a").guest("media-01", &["radarr.service", "sonarr.service"]);
    let b = Top::new("b").guest("media-01", &["radarr.service"]);
    let l = live(&[
        (HOST, &[]),
        ("media-01", &["radarr.service", "sonarr.service"]),
    ]);
    let d = compare(&a.load(), &b.load(), Some(&l));
    assert_eq!(d.losses, vec![unit("media-01", "sonarr.service")]);
}

#[test]
fn a_masked_unit_in_the_new_generation_is_lost() {
    let a = Top::new("a").unit("auditd.service");
    let b = Top::new("b");
    std::os::unix::fs::symlink("/dev/null", b.dir.join("etc/systemd/system/auditd.service"))
        .unwrap();
    let d = compare(&a.load(), &b.load(), None);
    assert_eq!(d.losses, vec![unit(HOST, "auditd.service")]);
}

const OLD_INPUTS: &str = r#"{"homeserver-secrets":{"rev":"aaaaaaaa1","lastModified":1789000000},
  "treff":{"rev":"bbbbbbbb1","lastModified":1788000000}}"#;

#[test]
fn an_input_that_goes_back_is_a_loss() {
    let a = Top::new("a").inputs(OLD_INPUTS);
    let b = Top::new("b").inputs(
        r#"{"homeserver-secrets":{"rev":"cccccccc0","lastModified":1788500000},
            "treff":{"rev":"dddddddd2","lastModified":1788900000}}"#,
    );
    let d = compare(&a.load(), &b.load(), None);
    assert_eq!(d.losses.len(), 1);
    assert!(
        matches!(&d.losses[0], Loss::Input { name, .. } if name == "homeserver-secrets"),
        "{:?}",
        d.losses
    );
}

#[test]
fn inputs_without_a_record_are_a_note_not_a_loss() {
    let a = Top::new("a");
    let b = Top::new("b").inputs(OLD_INPUTS);
    let d = compare(&a.load(), &b.load(), None);
    assert!(d.losses.is_empty());
    assert_eq!(d.notes, vec![Note::InputsUnknown { old: true }]);
}

#[test]
fn an_input_that_disappears_is_a_note() {
    let a = Top::new("a").inputs(OLD_INPUTS);
    let b = Top::new("b")
        .inputs(r#"{"homeserver-secrets":{"rev":"aaaaaaaa1","lastModified":1789000000}}"#);
    let d = compare(&a.load(), &b.load(), None);
    assert!(d.losses.is_empty());
    assert_eq!(
        d.notes,
        vec![Note::InputGone {
            name: "treff".into()
        }]
    );
}

#[test]
fn only_a_stale_tree_stops() {
    let a = Top::new("a").guest("req-01", &[]);
    let b = Top::new("b");
    let lossy = compare(&a.load(), &b.load(), None);
    let clean = compare(&b.load(), &b.load(), None);
    assert_eq!(decide(&clean, true, false), Verdict::Clean);
    assert_eq!(decide(&lossy, false, false), Verdict::Warn);
    assert_eq!(decide(&lossy, true, false), Verdict::Stop);
    assert_eq!(decide(&lossy, true, true), Verdict::Accepted);
    assert_eq!(Verdict::Stop.exit_code(), 1);
    assert_eq!(Verdict::Warn.exit_code(), 0);
}

#[test]
fn systemctl_json_counts_what_runs() {
    let json = r#"[{"unit":"a.service","load":"loaded","active":"active","sub":"running","description":""},
      {"unit":"b.service","load":"loaded","active":"inactive","sub":"dead","description":""},
      {"unit":"c.service","load":"loaded","active":"activating","sub":"start","description":""},
      {"unit":"d.service","load":"loaded","active":"failed","sub":"failed","description":""}]"#;
    let got: Vec<String> = parse_units(json).unwrap().into_iter().collect();
    assert_eq!(got, vec!["a.service", "c.service"]);
}

#[test]
fn machinectl_json_lists_containers() {
    let json = r#"[{"machine":"auth-01","class":"container","service":"systemd-nspawn","os":"nixos","version":"26.05","addresses":""},
      {"machine":"vm1","class":"vm","service":"qemu","os":"","version":"","addresses":""}]"#;
    assert_eq!(parse_machines(json).unwrap(), vec!["auth-01"]);
}

#[test]
fn a_guest_that_does_not_answer_falls_back_to_its_declaration() {
    let a = Top::new("a").guest("jelly-01", &["jellyfin.service", "alt.service"]);
    let b = Top::new("b").guest("jelly-01", &["jellyfin.service"]);
    let mut l = live(&[(HOST, &[])]);
    l.machines.insert("jelly-01".into());
    l.unreadable
        .insert("jelly-01".into(), "Failed to connect to bus".into());
    let d = compare(&a.load(), &b.load(), Some(&l));
    assert_eq!(d.losses, vec![unit("jelly-01", "alt.service")]);
    assert!(
        matches!(&d.notes[..], [Note::InputsUnknown { .. }, Note::LiveUnreadable { system, .. }] if system == "jelly-01")
    );
}

#[test]
fn a_running_guest_whose_removal_is_a_loss_even_without_units() {
    let a = Top::new("a").guest("req-01", &[]);
    let b = Top::new("b");
    let mut l = live(&[(HOST, &[])]);
    l.machines.insert("req-01".into());
    let d = compare(&a.load(), &b.load(), Some(&l));
    assert_eq!(d.losses, vec![guest("req-01")]);
}
