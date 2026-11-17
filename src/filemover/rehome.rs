use std::path::{Path, PathBuf};

use super::dates::destination_for;
use super::naming::NameRegistry;
use super::outcome::BatchOutcome;
use super::safe_fs::transfer_into;
use super::{Operation, Progress};
use crate::exif::CreationDate;

/// A verified re-home plan for one mismatched file: move it from where it
/// sits to the folder its EXIF date implies.
#[derive(Debug)]
pub struct RehomePlan {
    pub path: PathBuf,
    pub target: PathBuf,
    pub exif_date: String,
}

/// Compute re-home plans from verify mismatches. The target folder is derived
/// from the file's true EXIF date, keeping the description of the folder it
/// currently lives in.
pub fn plan_rehome(mismatches: &[(PathBuf, String)], root: &Path, format: &str) -> Vec<RehomePlan> {
    mismatches
        .iter()
        .filter_map(|(path, date_str)| {
            let date = CreationDate::DateCreated(date_str.clone());
            destination_for(path, &date, root, format).map(|target| RehomePlan {
                path: path.clone(),
                target,
                exif_date: date_str.clone(),
            })
        })
        .collect()
}

/// Super-safe re-home: for every plan, **copy** the file to its target,
/// verify the destination is byte-identical to the source, and only then
/// remove the source. Any failure leaves the source untouched. Identical
/// names at the destination are auto-renamed, never overwritten.
pub fn rehome_verified(plans: &[RehomePlan], progress: Progress) -> BatchOutcome {
    let mut outcome = BatchOutcome::planned(plans.len());
    let mut names = NameRegistry::new();

    for (i, plan) in plans.iter().enumerate() {
        progress(i + 1, plans.len());

        // A plan whose target resolves to its own current path is a no-op.
        if plan.path == plan.target {
            outcome.moved += 1;
            continue;
        }
        let Some(dir) = plan.target.parent() else {
            outcome.fail(&plan.path, "target has no parent directory");
            continue;
        };

        outcome.record(transfer_into(&plan.path, dir, &mut names, Operation::Move));
    }

    outcome
}
