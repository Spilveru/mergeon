//! Port de `electron/services/git-operations.test.js`, con repositorios git
//! reales en directorios temporales, más casos propios de este port.

pub(crate) mod support;

mod batch_sequence;
mod branches;
mod cleanup;
mod fixup;
mod misc;
mod network;
mod pull_requests;
mod rebase;
mod results;
mod sequence;
mod stash;
mod stash_options;
mod undo;
mod working_tree;
mod worktrees;
