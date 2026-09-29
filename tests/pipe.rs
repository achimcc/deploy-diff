//! B110: a report longer than the pipe buffer, to a reader that has gone.
//! Until 0.1.2 SIGPIPE killed the process (no exit code at all); with Rust's
//! default it would panic (101). Either way the verdict was lost.

use std::fs;
use std::io::Read;
use std::process::{Command, Stdio};

#[test]
fn a_closed_pipe_ends_with_the_verdict_not_a_signal_or_a_panic() {
    let dir = std::env::temp_dir().join(format!("deploy-diff-pipe-{}", std::process::id()));
    let (old, new) = (dir.join("old"), dir.join("new"));
    fs::create_dir_all(old.join("etc/systemd/system")).unwrap();
    fs::create_dir_all(new.join("etc/systemd/system")).unwrap();
    // 3000 lost units, ~70 bytes each: far beyond the 64 KiB of a pipe.
    for i in 0..3000 {
        fs::write(
            old.join(format!(
                "etc/systemd/system/a-rather-long-unit-name-to-fill-the-pipe-{i:05}.service"
            )),
            "[Service]\nExecStart=/bin/true\n",
        )
        .unwrap();
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_deploy-diff"))
        .args(["compare"])
        .arg(&old)
        .arg(&new)
        .arg("--stale")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    let status = child.wait().unwrap();
    let _ = fs::remove_dir_all(&dir);

    assert!(!err.contains("panicked"), "{err}");
    assert_eq!(status.code(), Some(1), "{status:?} {err}");
}
