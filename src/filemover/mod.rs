//! Moving, copying and organizing media files on disk.
//!
//! Every operation that can lose data goes through [`safe_fs`]: copies are
//! byte-verified before a source is removed, destinations are never
//! overwritten, and name collisions are resolved by [`naming`].
//!
//! The submodules are grouped by workflow; their public items are re-exported
//! here so callers use `crate::filemover::...` regardless of where an item
//! lives.

mod consolidate;
mod dates;
mod dedupe;
mod duplicates;
mod error;
mod library;
mod naming;
mod organize;
mod outcome;
mod purgatory;
mod rehome;
mod safe_fs;
mod similar;
#[cfg(test)]
mod test_support;
mod transfer;

pub use consolidate::{
    consolidate_events, consolidate_events_with_progress, default_event_purgatory,
    preview_consolidate_events, preview_consolidate_events_with_progress, ConsolidationEntry,
    ConsolidationOutcome, ConsolidationPlan,
};
pub use dates::{destination_for, render_date_folder};
pub use dedupe::{find_duplicates, merge_dirs, relocate_tree};
pub use error::FileMoverError;
pub use library::{
    default_purgatory, merge_libraries, merge_libraries_with_progress, preview_merge,
    preview_merge_with_progress, DuplicateGroup, MergeOutcome, MergePlan, MovePlan,
};
pub use organize::{organize_by_date, organize_into_destination_with_progress, OrganizeOutcome};
pub use outcome::{BatchOutcome, PurgatoryOutcome, RehomeOutcome, TransferOutcome};
pub use purgatory::{move_to_purgatory_with_progress, purge_proven_duplicates_with_progress};
pub use rehome::{plan_rehome, rehome_verified, RehomePlan};
pub use safe_fs::TransferError;
pub use similar::{
    apply_similar_decisions_with_progress, swap_better_copies_with_progress, DecisionOutcome,
    SimilarDecision, SwapOutcome,
};
pub use transfer::{
    copy_originals, copy_originals_with_progress, transfer_dated_with_progress,
    transfer_files_with_progress, transfer_originals_with_progress, CopyOutcome,
};

/// Whether a transfer keeps the source (`Copy`) or removes it once the
/// destination is verified (`Move`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Move,
    Copy,
}

/// How [`find_duplicates`] decides two files are the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateStrategy {
    /// Identical bytes.
    ExactHash,
    /// Identical encoded image data (metadata ignored), falling back to the
    /// whole file when the format is not recognized.
    ImageData,
}

pub use crate::Progress;
