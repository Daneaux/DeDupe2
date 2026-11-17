use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::dates::is_event_name;
use super::duplicates::confirmed_groups;
use super::library::MovePlan;
use super::naming::NameRegistry;
use super::safe_fs::{relocate_file, remove_if_empty};
use super::{FileMoverError, Operation, Progress};
use crate::image_reader::collect_image_files;

#[derive(Debug)]
pub struct ConsolidationOutcome {
    pub folded_groups: usize,
    pub purged_files: usize,
    pub moved_files: usize,
}

#[derive(Debug)]
pub struct ConsolidationPlan {
    pub purgatory: PathBuf,
    pub entries: Vec<ConsolidationEntry>,
}

#[derive(Debug)]
pub struct ConsolidationEntry {
    pub keeper: String,
    pub folded: Vec<String>,
    pub moved: Vec<MovePlan>,
    pub purged: Vec<MovePlan>,
}

struct EventFolder {
    path: PathBuf,
    name: String,
}

pub fn preview_consolidate_events(
    dir: &Path,
    purgatory: Option<&Path>,
) -> Result<ConsolidationPlan, FileMoverError> {
    preview_consolidate_events_with_progress(dir, purgatory, &|_, _| {})
}

pub fn preview_consolidate_events_with_progress(
    dir: &Path,
    purgatory: Option<&Path>,
    progress: Progress,
) -> Result<ConsolidationPlan, FileMoverError> {
    consolidate_plan(dir, purgatory, progress)
}

pub fn consolidate_events(
    dir: &Path,
    purgatory: Option<&Path>,
) -> Result<ConsolidationOutcome, FileMoverError> {
    consolidate_events_with_progress(dir, purgatory, &|_, _| {})
}

pub fn consolidate_events_with_progress(
    dir: &Path,
    purgatory: Option<&Path>,
    progress: Progress,
) -> Result<ConsolidationOutcome, FileMoverError> {
    let plan = consolidate_plan(dir, purgatory, progress)?;

    let moves_total: usize = plan
        .entries
        .iter()
        .map(|e| e.moved.len() + e.purged.len())
        .sum();

    let mut outcome = ConsolidationOutcome {
        folded_groups: 0,
        purged_files: 0,
        moved_files: 0,
    };
    let mut done = 0usize;

    for entry in &plan.entries {
        for m in &entry.moved {
            relocate_file(&m.source, &m.target, Operation::Move)?;
            outcome.moved_files += 1;
            done += 1;
            progress(done, moves_total);
        }
        for m in &entry.purged {
            relocate_file(&m.source, &m.target, Operation::Move)?;
            outcome.purged_files += 1;
            done += 1;
            progress(done, moves_total);
        }
        for name in &entry.folded {
            remove_if_empty(&dir.join(name));
        }
        outcome.folded_groups += 1;
    }

    Ok(outcome)
}

fn consolidate_plan(
    dir: &Path,
    purgatory: Option<&Path>,
    progress: Progress,
) -> Result<ConsolidationPlan, FileMoverError> {
    let purgatory = match purgatory {
        Some(p) => p.to_path_buf(),
        None => default_event_purgatory(dir),
    };

    let groups = group_event_folders(dir)?;
    let mut entries = Vec::new();

    for mut group in groups {
        if group.len() < 2 {
            continue;
        }

        group.sort_by(|a, b| {
            b.name
                .len()
                .cmp(&a.name.len())
                .then_with(|| a.name.cmp(&b.name))
        });
        let keeper = &group[0];

        let mut files: Vec<(PathBuf, usize)> = Vec::new();
        for (idx, folder) in group.iter().enumerate() {
            for p in collect_image_files(&folder.path)
                .map_err(|e| FileMoverError::Other(e.to_string()))?
            {
                files.push((p, idx));
            }
        }

        let abs: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
        // Purge only files whose full content matches the group's keeper.
        let purged: HashSet<usize> = confirmed_groups(&abs, progress)
            .into_iter()
            .flat_map(|group| group.into_iter().skip(1))
            .collect();

        let mut keep_names = NameRegistry::new();
        let mut purge_names = NameRegistry::new();

        let mut moved = Vec::new();
        let mut purged_plans = Vec::new();

        for (i, (path, origin)) in files.iter().enumerate() {
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();

            if purged.contains(&i) {
                let dest_dir = purgatory.join(&group[*origin].name);
                let unique = purge_names.claim(&dest_dir, &filename);
                purged_plans.push(MovePlan {
                    source: path.clone(),
                    target: dest_dir.join(&unique),
                });
            } else if *origin != 0 {
                let unique = keep_names.claim(&keeper.path, &filename);
                moved.push(MovePlan {
                    source: path.clone(),
                    target: keeper.path.join(&unique),
                });
            }
        }

        let folded: Vec<String> = group[1..].iter().map(|f| f.name.clone()).collect();
        entries.push(ConsolidationEntry {
            keeper: keeper.name.clone(),
            folded,
            moved,
            purged: purged_plans,
        });
    }

    Ok(ConsolidationPlan { purgatory, entries })
}

pub fn default_event_purgatory(dir: &Path) -> PathBuf {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("photos");
    dir.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{name}-purgatory"))
}

fn group_event_folders(dir: &Path) -> Result<Vec<Vec<EventFolder>>, FileMoverError> {
    let mut groups: HashMap<String, Vec<EventFolder>> = HashMap::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_event_name(&name) {
            continue;
        }
        let key = name[..5].to_string();
        groups.entry(key).or_default().push(EventFolder {
            path: entry.path(),
            name,
        });
    }

    let mut list: Vec<Vec<EventFolder>> = groups.into_values().collect();
    list.sort_by(|a, b| a[0].name.cmp(&b[0].name));
    Ok(list)
}
