//! Warn or stop.
//!
//! A loss from a tree that contains what is running is a decision someone
//! made in that tree — a guest abolished, a unit renamed. A loss from a tree
//! that does NOT contain the running commit is what another session rolled
//! out, taken away again. Only the second stops the deploy, and even then a
//! person can let it through: a rollback is exactly that, on purpose.

use crate::diff::Diff;

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing lost.
    Clean,
    /// Losses, but the tree contains the running commit.
    Warn,
    /// Losses from a stale tree, let through by the operator.
    Accepted,
    /// Losses from a stale tree.
    Stop,
}

impl Verdict {
    pub fn exit_code(&self) -> u8 {
        match self {
            Verdict::Stop => 1,
            _ => 0,
        }
    }
}

/// `stale`: the tree does not contain the running commit (or that commit is
/// unknown, or the running generation was built from a dirty tree — its
/// content is then in no commit at all).
pub fn decide(diff: &Diff, stale: bool, accept: bool) -> Verdict {
    match (diff.losses.is_empty(), stale, accept) {
        (true, _, _) => Verdict::Clean,
        (false, false, _) => Verdict::Warn,
        (false, true, true) => Verdict::Accepted,
        (false, true, false) => Verdict::Stop,
    }
}
