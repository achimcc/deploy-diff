#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use deploy_diff::diff::{self, Loss};
use deploy_diff::live;
use deploy_diff::output;
use deploy_diff::snapshot::Snapshot;
use deploy_diff::text::visible;
use deploy_diff::verdict::{self, Verdict};

const USAGE: &str = "\
deploy-diff — what does this deploy take away?

USAGE:
  deploy-diff compare OLD NEW [--live] [--stale] [--accept]

  OLD, NEW   two NixOS toplevels: the running one and the one to activate
  --live     count only what runs now (asks systemd; root, on the target)
  --stale    the tree NEW was built from does not contain OLD's commit
  --accept   let losses from a stale tree through (a deliberate rollback)

Exit 0: nothing lost, or losses from a tree that contains the running commit
(a warning), or accepted. Exit 1: losses from a stale tree. Exit 2: error.";

struct Args {
    old: PathBuf,
    new: PathBuf,
    live: bool,
    stale: bool,
    accept: bool,
}

fn parse() -> Result<Args, lexopt::Error> {
    use lexopt::prelude::*;
    let mut p = lexopt::Parser::from_env();
    let mut pos: Vec<PathBuf> = Vec::new();
    let (mut live, mut stale, mut accept) = (false, false, false);
    let mut cmd = None;
    while let Some(arg) = p.next()? {
        match arg {
            Long("live") => live = true,
            Long("stale") => stale = true,
            Long("accept") => accept = true,
            Short('h') | Long("help") => {
                let code = output::finish(&mut std::io::stdout().lock(), &format!("{USAGE}\n"), 0);
                std::process::exit(code.into());
            }
            Value(v) if cmd.is_none() => cmd = Some(v.string()?),
            Value(v) => pos.push(v.into()),
            _ => return Err(arg.unexpected()),
        }
    }
    if cmd.as_deref() != Some("compare") || pos.len() != 2 {
        return Err("expected: compare OLD NEW".into());
    }
    let new = pos.pop().expect("two");
    let old = pos.pop().expect("two");
    Ok(Args {
        old,
        new,
        live,
        stale,
        accept,
    })
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("deploy-diff: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let load = |p: &PathBuf| Snapshot::load(p);
    let (old, new) = match (load(&args.old), load(&args.new)) {
        (Ok(o), Ok(n)) => (o, n),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("deploy-diff: {e}");
            return ExitCode::from(2);
        }
    };
    let live = if args.live {
        match live::collect() {
            Ok(l) => Some(l),
            Err(e) => {
                eprintln!("deploy-diff: live state: {}", visible(&e));
                return ExitCode::from(2);
            }
        }
    } else {
        None
    };

    let d = diff::compare(&old, &new, live.as_ref());
    let v = verdict::decide(&d, args.stale, args.accept);

    let guests = d
        .losses
        .iter()
        .filter(|l| matches!(l, Loss::Guest { .. }))
        .count();
    let units = d
        .losses
        .iter()
        .filter(|l| matches!(l, Loss::Unit { .. }))
        .count();
    let inputs = d.losses.len() - guests - units;
    // The report is written at once at the end; `deploy-diff … | head` ends
    // quietly with the verdict, it does not panic on a closed pipe (B110).
    let mut out = String::new();
    let _ = writeln!(
        out,
        "deploy-diff: {} system(s) before, {} after — {guests} guest(s), {units} unit(s), {inputs} input(s) lost{}",
        old.systems.len(),
        new.systems.len(),
        if args.live {
            ""
        } else {
            " (declared, not live)"
        }
    );
    for l in &d.losses {
        // Names and error texts can come from a guest (B81): no control
        // character reaches the terminal.
        let _ = writeln!(out, "  LOST  {}", visible(&l.to_string()));
    }
    for n in &d.notes {
        let _ = writeln!(out, "  note  {}", visible(&n.to_string()));
    }
    match v {
        Verdict::Clean => {}
        Verdict::Warn => out.push_str(
            "WARNING: the tree contains the running commit, so these losses were made in it on purpose — or check them.\n",
        ),
        Verdict::Accepted => out.push_str("ACCEPTED: losses from a stale tree, let through.\n"),
        Verdict::Stop => out.push_str(
            "STOP: the tree does not contain the running commit — this deploy takes away what another one rolled out.\n\
             Merge the running commit, or let it through on purpose (--accept).\n",
        ),
    }
    ExitCode::from(output::finish(
        &mut std::io::stdout().lock(),
        &out,
        v.exit_code(),
    ))
}
