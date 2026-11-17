use std::collections::HashMap;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use super::naming::{file_name, name_len, NameRegistry};
use super::safe_fs::relocate_file;
use super::{FileMoverError, Operation, Progress};
use crate::image_reader::is_supported_image;
use crate::Scanner::scan::hash_image_data_parallel;

#[derive(Debug)]
pub struct MergeOutcome {
    pub kept: Vec<PathBuf>,
    pub purged: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct MovePlan {
    pub source: PathBuf,
    pub target: PathBuf,
}

#[derive(Debug)]
pub struct DuplicateGroup {
    pub kept: MovePlan,
    pub purged: Vec<MovePlan>,
}

#[derive(Debug)]
pub struct MergePlan {
    pub destination: PathBuf,
    pub purgatory: PathBuf,
    pub duplicate_groups: Vec<DuplicateGroup>,
    pub uniques: Vec<MovePlan>,
}

struct SourceFile {
    path: PathBuf,
    rel: PathBuf,
}

pub fn preview_merge(
    sources: &[PathBuf],
    destination: &Path,
    purgatory: Option<&Path>,
) -> Result<MergePlan, FileMoverError> {
    preview_merge_with_progress(sources, destination, purgatory, &|_, _| {})
}

pub fn preview_merge_with_progress(
    sources: &[PathBuf],
    destination: &Path,
    purgatory: Option<&Path>,
    progress: Progress,
) -> Result<MergePlan, FileMoverError> {
    plan_merge(sources, destination, purgatory, progress)
}

pub fn merge_libraries(
    sources: &[PathBuf],
    destination: &Path,
    purgatory: Option<&Path>,
    op: Operation,
) -> Result<MergeOutcome, FileMoverError> {
    merge_libraries_with_progress(sources, destination, purgatory, op, &|_, _| {})
}

pub fn merge_libraries_with_progress(
    sources: &[PathBuf],
    destination: &Path,
    purgatory: Option<&Path>,
    op: Operation,
    progress: Progress,
) -> Result<MergeOutcome, FileMoverError> {
    let plan = plan_merge(sources, destination, purgatory, progress)?;

    let moved_total = plan.duplicate_groups.len()
        + plan
            .duplicate_groups
            .iter()
            .map(|g| g.purged.len())
            .sum::<usize>()
        + plan.uniques.len();

    let mut kept = Vec::new();
    let mut purged = Vec::new();
    let mut done = 0usize;

    for group in &plan.duplicate_groups {
        relocate_file(&group.kept.source, &group.kept.target, op)?;
        done += 1;
        progress(done, moved_total);
        kept.push(group.kept.target.clone());
        for p in &group.purged {
            relocate_file(&p.source, &p.target, op)?;
            done += 1;
            progress(done, moved_total);
            purged.push(p.target.clone());
        }
    }
    for u in &plan.uniques {
        relocate_file(&u.source, &u.target, op)?;
        done += 1;
        progress(done, moved_total);
        kept.push(u.target.clone());
    }

    Ok(MergeOutcome { kept, purged })
}

fn plan_merge(
    sources: &[PathBuf],
    destination: &Path,
    purgatory: Option<&Path>,
    progress: Progress,
) -> Result<MergePlan, FileMoverError> {
    let purgatory = match purgatory {
        Some(p) => p.to_path_buf(),
        None => default_purgatory(sources),
    };

    let mut all: Vec<SourceFile> = Vec::new();
    for source in sources {
        collect_files(source, &mut all)?;
    }
    all.retain(|f| split_event(&f.rel).is_some());

    let paths: Vec<PathBuf> = all.iter().map(|f| f.path.clone()).collect();
    let hashes = hash_image_data_parallel(&paths, 64 * 1024, progress);

    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, fh) in hashes.into_iter().enumerate() {
        buckets.entry(fh.hash).or_default().push(i);
    }

    let mut groups: Vec<Vec<usize>> = Vec::new();
    for mut bucket in buckets.into_values() {
        bucket.sort_by(|&a, &b| {
            name_len(&all[a].path)
                .cmp(&name_len(&all[b].path))
                .then_with(|| all[a].path.cmp(&all[b].path))
        });
        groups.push(bucket);
    }
    groups.sort_by(|a, b| all[a[0]].path.cmp(&all[b[0]].path));

    let canonical = canonical_folders(&all);

    let mut dest_names = NameRegistry::new();
    let mut purg_names = NameRegistry::new();

    let mut duplicate_groups = Vec::new();
    let mut uniques = Vec::new();

    for group in &groups {
        let kept_index = group[0];
        let kept_source = all[kept_index].path.clone();
        let kept_target = dest_target(&all[kept_index], &canonical, destination, &mut dest_names)?;

        if group.len() == 1 {
            uniques.push(MovePlan {
                source: kept_source,
                target: kept_target,
            });
        } else {
            let purged = group[1..]
                .iter()
                .map(|&i| MovePlan {
                    source: all[i].path.clone(),
                    target: purg_target(&all[i], &purgatory, &mut purg_names),
                })
                .collect();
            duplicate_groups.push(DuplicateGroup {
                kept: MovePlan {
                    source: kept_source,
                    target: kept_target,
                },
                purged,
            });
        }
    }

    Ok(MergePlan {
        destination: destination.to_path_buf(),
        purgatory,
        duplicate_groups,
        uniques,
    })
}

/// Default purge location: a `purgatory` folder at the shared root (common
/// parent) of the source trees.
pub fn default_purgatory(sources: &[PathBuf]) -> PathBuf {
    let mut common = match sources.iter().find_map(|s| s.parent()) {
        Some(p) => p.to_path_buf(),
        None => return PathBuf::from("purgatory"),
    };

    for source in sources {
        let s = source.as_path();
        while !s.starts_with(&common) {
            match common.parent() {
                Some(p) => common = p.to_path_buf(),
                None => return PathBuf::from("purgatory"),
            }
        }
    }

    common.join("purgatory")
}

fn collect_files(source: &Path, out: &mut Vec<SourceFile>) -> Result<(), FileMoverError> {
    for entry in WalkDir::new(source).into_iter() {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if !is_supported_image(&path) {
            continue;
        }
        let rel = path
            .strip_prefix(source)
            .map_err(|e| FileMoverError::Other(e.to_string()))?
            .to_path_buf();
        out.push(SourceFile { path, rel });
    }
    Ok(())
}

fn split_event(rel: &Path) -> Option<(String, String, String)> {
    let file = file_name(rel);
    let parent = rel.parent()?;
    let mut comps = parent.components();
    let year = comps.next()?.as_os_str().to_string_lossy().into_owned();
    let event = comps.next()?.as_os_str().to_string_lossy().into_owned();
    if event.is_empty() {
        return None;
    }
    Some((year, event, file))
}

fn canonical_folders(all: &[SourceFile]) -> HashMap<(String, String), String> {
    let mut map: HashMap<(String, String), String> = HashMap::new();
    for file in all {
        if let Some((year, event, _)) = split_event(&file.rel) {
            let mmdd: String = event.chars().take(5).collect();
            map.entry((year, mmdd))
                .and_modify(|name| {
                    if event.len() > name.len() {
                        *name = event.clone();
                    }
                })
                .or_insert(event);
        }
    }
    map
}

fn destination_folder(
    rel: &Path,
    canonical: &HashMap<(String, String), String>,
) -> Option<PathBuf> {
    let (year, event, _) = split_event(rel)?;
    let mmdd: String = event.chars().take(5).collect();
    let name = canonical
        .get(&(year.clone(), mmdd))
        .cloned()
        .unwrap_or(event);
    Some(Path::new(&year).join(name))
}

fn dest_target(
    file: &SourceFile,
    canonical: &HashMap<(String, String), String>,
    destination: &Path,
    names: &mut NameRegistry,
) -> Result<PathBuf, FileMoverError> {
    let folder = destination_folder(&file.rel, canonical).ok_or_else(|| {
        FileMoverError::Other(format!(
            "no year/event structure for {}",
            file.path.display()
        ))
    })?;
    let dir = destination.join(folder);
    let unique = names.claim(&dir, &file_name(&file.rel));
    Ok(dir.join(unique))
}

fn purg_target(file: &SourceFile, purgatory: &Path, names: &mut NameRegistry) -> PathBuf {
    let parent = file.rel.parent().unwrap_or_else(|| Path::new(""));
    let dir = purgatory.join(parent);
    let unique = names.claim(&dir, &file_name(&file.rel));
    dir.join(unique)
}
