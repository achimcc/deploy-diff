//! The comparison against small toplevels built on disk: the same layout
//! NixOS writes (`etc/systemd/system`, `etc/nixos-containers/*.conf`).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use deploy_diff::diff::{Loss, Note, compare};
use deploy_diff::live::{
    Live, PROGRAMS, guest_active, guest_payload, host_view, parse_machines, parse_units,
};
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

/// B81: a guest's systemd reports a unit name full of escapes. It is not a
/// name systemd accepts, so it is not a unit, and it never reaches the report.
#[test]
fn a_unit_name_systemd_would_never_accept_is_dropped() {
    let json = r#"[{"unit":"a.service","load":"loaded","active":"active","sub":"running","description":""},
      {"unit":"b@\u001b]52;c;S0FOQVJJRQ==\u0007x.service","load":"loaded","active":"active","sub":"running","description":""}]"#;
    let got: Vec<String> = parse_units(json).unwrap().into_iter().collect();
    assert_eq!(got, vec!["a.service"]);
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

/// A cgroup v2 tree the way the kernel lays it out for an nspawn guest: the
/// payload below the container's unit, the guest's systemd in `init.scope`,
/// its units in `system.slice` (instances in their own sub-slice), and a
/// `cgroup.events` in each.
struct Cgroups {
    root: PathBuf,
}

const PAYLOAD: &str = "/machine.slice/container@media-01.service/payload";

impl Cgroups {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "deploy-diff-cg-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        let c = Cgroups { root };
        c.group("init.scope", true);
        c
    }

    fn group(&self, below_payload: &str, populated: bool) -> &Self {
        let dir = self
            .root
            .join(PAYLOAD.trim_start_matches('/'))
            .join(below_payload);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("cgroup.events"),
            format!("populated {}\nfrozen 0\n", u8::from(populated)),
        )
        .unwrap();
        self
    }
}

impl Drop for Cgroups {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// B109: a guest whose systemd reports no units at all. The host still sees
/// sonarr's cgroup populated below the guest's payload, so the loss counts
/// and a stale tree stops — the guest cannot talk the STOP away.
#[test]
fn a_guest_that_reports_nothing_cannot_hide_a_running_unit() {
    let cg = Cgroups::new();
    cg.group("system.slice", true)
        .group("system.slice/sonarr.service", true)
        .group("system.slice/radarr.service", true)
        .group("system.slice/idle.service", false)
        .group("system.slice/system-getty.slice/getty@tty1.service", true);

    let payload =
        guest_payload(&format!("0::{PAYLOAD}/init.scope\n")).expect("leader in init.scope");
    let seen = host_view(&cg.root, &payload).unwrap();
    assert_eq!(
        seen.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["getty@tty1.service", "radarr.service", "sonarr.service"]
    );

    let a = Top::new("a").guest(
        "media-01",
        &["radarr.service", "sonarr.service", "idle.service"],
    );
    let b = Top::new("b").guest("media-01", &["radarr.service"]);
    let mut l = live(&[(HOST, &[])]);
    l.machines.insert("media-01".into());
    let reported = parse_units("[]");
    l.active
        .insert("media-01".into(), guest_active(reported, Ok(seen)).unwrap());
    let d = compare(&a.load(), &b.load(), Some(&l));
    assert_eq!(d.losses, vec![unit("media-01", "sonarr.service")]);
    assert_eq!(decide(&d, true, false), Verdict::Stop);
}

/// B109: without the host's view the guest counts by its declaration — a
/// guest cannot shrink its losses by breaking the second witness.
#[test]
fn a_guest_the_host_cannot_see_into_falls_back_to_its_declaration() {
    let got = guest_active(Ok(BTreeSet::new()), Err("no leader".into()));
    assert!(got.unwrap_err().contains("host cgroup view"));
    let missing = std::env::temp_dir().join("deploy-diff-no-such-cgroup-tree");
    assert!(host_view(&missing, PAYLOAD).is_err());
}

/// The leader is trusted only in `<payload>/init.scope`, and a path that
/// walks up the host's tree is refused.
#[test]
fn the_payload_comes_from_the_leaders_init_scope() {
    assert_eq!(
        guest_payload("0::/machine.slice/systemd-nspawn@x.service/payload/init.scope\n").unwrap(),
        "/machine.slice/systemd-nspawn@x.service/payload"
    );
    for bad in [
        "0::/machine.slice/container@x.service/payload\n",
        "0::/machine.slice/../system.slice/init.scope\n",
        "0::init.scope\n",
        "12:pids:/x/init.scope\n",
        "",
    ] {
        assert!(guest_payload(bad).is_err(), "{bad:?}");
    }
}

/// B110: systemctl and machinectl come from the running system, not PATH.
#[test]
fn programs_are_absolute_paths_of_the_running_system() {
    for p in PROGRAMS {
        assert!(p.starts_with("/run/current-system/sw/bin/"), "{p}");
    }
}
