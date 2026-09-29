//! deploy-diff — what does this deploy take away?
//!
//! A deploy from a stale tree does not fail. It activates, exits 0, and the
//! host runs a generation without the guest, the unit or the secret another
//! session rolled out an hour earlier. A removed service is not a failed one:
//! `systemctl --failed` stays empty. This crate compares the generation about
//! to be activated with the one that is running and names the losses.

#![forbid(unsafe_code)]

pub mod diff;
pub mod live;
pub mod output;
pub mod snapshot;
pub mod text;
pub mod verdict;
