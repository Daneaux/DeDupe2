use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::Scanner::fastScan::hash_image_data_parallel;
use crate::Scanner::scanner::ScannedFile;
use crate::exif::{creation_date, CreationDate};
use crate::image_reader::{hash_all_bytes, hash_image_data, is_supported_image, ReadLimit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Move,
    Copy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateStrategy {
    ExactHash,
    ImageData,
}

#[derive(Debug)]
pub enum FileMoverError {
    Io(std::io::Error),
    Other(String),
}

impl fmt::Display for FileMoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileMoverError::Io(e) => write!(f, "io error: {e}"),
            FileMoverError::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for FileMoverError {}

impl From<std::io::Error> for FileMoverError {
    fn from(e: std::io::Error) -> Self {
        FileMoverError::Io(e)
    }
}

impl From<walkdir::Error> for FileMoverError {
    fn from(e: walkdir::Error) -> Self {
        FileMoverError::Other(e.to_string())
    }
}

impl From<crate::image_reader::ImageReaderError> for FileMoverError {
    fn from(e: crate::image_reader::ImageReaderError) -> Self {
        FileMoverError::Other(e.to_string())
    }
}

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
    progress: &(dyn Fn(usize, usize) + Send + Sync),
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
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<MergeOutcome, FileMoverError> {
    let plan = plan_merge(sources, destination, purgatory, progress)?;

    let moved_total = plan.duplicate_groups.len()
        + plan.duplicate_groups.iter().map(|g| g.purged.len()).sum::<usize>()
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
    progress: &(dyn Fn(usize, usize) + Send + Sync),
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

    let mut dest_taken: HashSet<String> = HashSet::new();
    let mut purg_taken: HashSet<String> = HashSet::new();

    let mut duplicate_groups = Vec::new();
    let mut uniques = Vec::new();

    for group in &groups {
        let kept_index = group[0];
        let kept_source = all[kept_index].path.clone();
        let kept_target = dest_target(&all[kept_index], &canonical, destination, &mut dest_taken)?;

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
                    target: purg_target(&all[i], &purgatory, &mut purg_taken),
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

fn name_len(path: &Path) -> usize {
    path.file_name().map(|n| n.len()).unwrap_or(0)
}

fn file_name(rel: &Path) -> String {
    rel.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string()
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
    taken: &mut HashSet<String>,
) -> Result<PathBuf, FileMoverError> {
    let filename = file_name(&file.rel);
    let unique = unique_name(&filename, taken);
    taken.insert(unique.clone());

    let folder = destination_folder(&file.rel, canonical).ok_or_else(|| {
        FileMoverError::Other(format!("no year/event structure for {}", file.path.display()))
    })?;
    Ok(destination.join(folder).join(unique))
}

fn purg_target(file: &SourceFile, purgatory: &Path, taken: &mut HashSet<String>) -> PathBuf {
    let filename = file_name(&file.rel);
    let unique = unique_name(&filename, taken);
    taken.insert(unique.clone());

    let parent = file.rel.parent().unwrap_or_else(|| Path::new(""));
    purgatory.join(parent).join(unique)
}

pub fn relocate_tree(src: &Path, dst: &Path, op: Operation) -> Result<(), FileMoverError> {
    for entry in WalkDir::new(src) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(src)
            .map_err(|e| FileMoverError::Other(e.to_string()))?;
        let target = dst.join(rel);
        relocate_file(entry.path(), &target, op)?;
    }
    Ok(())
}

pub fn organize_by_date(
    files: &[ScannedFile],
    dst_root: &Path,
    source_name: &str,
    op: Operation,
) -> Result<(), FileMoverError> {
    let event = extract_event(source_name);
    let mut taken: HashSet<String> = HashSet::new();

    for file in files {
        let folder = match parse_date(&file.creation_date) {
            Some((year, month, day)) => {
                let name = if event.is_empty() {
                    format!("{month:02}-{day:02}")
                } else {
                    format!("{month:02}-{day:02}-{event}")
                };
                dst_root.join(format!("{year:04}")).join(name)
            }
            None => dst_root.join("unknown"),
        };

        let filename = file
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");
        let unique = unique_name(filename, &taken);
        taken.insert(unique.clone());

        let target = folder.join(unique);
        relocate_file(&file.path, &target, op)?;
    }

    Ok(())
}

pub fn find_duplicates(
    paths: &[PathBuf],
    strategy: DuplicateStrategy,
) -> Result<Vec<Vec<PathBuf>>, FileMoverError> {
    let mut groups: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for path in paths {
        let hash = hash_for_strategy(path, strategy)?;
        groups.entry(hash).or_default().push(path.clone());
    }

    Ok(groups
        .into_values()
        .filter(|group| group.len() >= 2)
        .collect())
}

pub fn merge_dirs(
    dir_a: &Path,
    dir_b: &Path,
    dst: &Path,
    op: Operation,
    strategy: DuplicateStrategy,
) -> Result<(), FileMoverError> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_shallow(dir_a, &mut files)?;
    collect_shallow(dir_b, &mut files)?;

    let dupes = find_duplicates(&files, strategy)?;

    let mut discarded: HashSet<PathBuf> = HashSet::new();
    for set in &dupes {
        let best = select_best(set)?;
        for path in set {
            if *path != *best {
                discarded.insert(path.clone());
            }
        }
    }

    let mut taken: HashSet<String> = HashSet::new();
    for path in &files {
        if discarded.contains(path) {
            continue;
        }
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let unique = unique_name(filename, &taken);
        taken.insert(unique.clone());

        let target = dst.join(unique);
        relocate_file(path, &target, op)?;
    }

    Ok(())
}

fn hash_for_strategy(path: &Path, strategy: DuplicateStrategy) -> Result<u64, FileMoverError> {
    match strategy {
        DuplicateStrategy::ExactHash => Ok(hash_all_bytes(path)?),
        DuplicateStrategy::ImageData => match hash_image_data(path, ReadLimit::All) {
            Ok(hash) => Ok(hash),
            Err(_) => Ok(hash_all_bytes(path)?),
        },
    }
}

fn select_best(paths: &[PathBuf]) -> Result<&PathBuf, FileMoverError> {
    let mut best = &paths[0];
    let mut best_size = std::fs::metadata(best)?.len();
    let mut best_name_len = best.file_name().map(|n| n.len()).unwrap_or(usize::MAX);

    for p in &paths[1..] {
        let size = std::fs::metadata(p)?.len();
        let name_len = p.file_name().map(|n| n.len()).unwrap_or(usize::MAX);
        if size > best_size || (size == best_size && name_len < best_name_len) {
            best = p;
            best_size = size;
            best_name_len = name_len;
        }
    }

    Ok(best)
}

fn collect_shallow(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), FileMoverError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn relocate_file(src: &Path, dst: &Path, op: Operation) -> Result<(), FileMoverError> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match op {
        Operation::Move => move_file(src, dst)?,
        Operation::Copy => {
            std::fs::copy(src, dst)?;
        }
    }
    Ok(())
}

fn move_file(src: &Path, dst: &Path) -> Result<(), FileMoverError> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(src, dst)?;
            std::fs::remove_file(src)?;
            Ok(())
        }
    }
}

fn parse_date(date: &CreationDate) -> Option<(u32, u32, u32)> {
    let s = match date {
        CreationDate::DateCreated(v) => v,
        CreationDate::Unknown => return None,
    };

    let digits: Vec<u32> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;

    if digits.len() < 3 {
        return None;
    }
    let (year, month, day) = (digits[0], digits[1], digits[2]);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

fn extract_event(source_name: &str) -> String {
    let mut words: Vec<&str> = source_name
        .split(|c: char| c == ' ' || c == '-' || c == '_')
        .filter(|w| !w.is_empty())
        .collect();

    while let Some(first) = words.first() {
        if is_date_token(first) {
            words.remove(0);
        } else {
            break;
        }
    }

    words.join("-")
}

fn is_date_token(word: &str) -> bool {
    if word.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }

    const MONTHS: [&str; 24] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
        "january", "february", "march", "april", "may", "june", "july", "august", "september",
        "october", "november", "december",
    ];
    MONTHS.contains(&word.to_lowercase().as_str())
}

fn unique_name(filename: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(filename) {
        return filename.to_string();
    }

    let (stem, ext) = match filename.rfind('.') {
        Some(idx) => (&filename[..idx], &filename[idx..]),
        None => (filename, ""),
    };

    let mut n = 1;
    loop {
        let candidate = format!("{stem} ({n}){ext}");
        if !taken.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

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
    progress: &(dyn Fn(usize, usize) + Send + Sync),
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
    progress: &(dyn Fn(usize, usize) + Send + Sync),
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
            try_remove_empty(&dir.join(name));
        }
        outcome.folded_groups += 1;
    }

    Ok(outcome)
}

fn consolidate_plan(
    dir: &Path,
    purgatory: Option<&Path>,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
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
            for p in collect_image_files(&folder.path)? {
                files.push((p, idx));
            }
        }

        let abs: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
        let hashes = hash_image_data_parallel(&abs, 64 * 1024, progress);

        let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
        for (i, fh) in hashes.into_iter().enumerate() {
            buckets.entry(fh.hash).or_default().push(i);
        }

        let mut purged: HashSet<usize> = HashSet::new();
        for bucket in buckets.values_mut() {
            bucket.sort_by(|&a, &b| {
                name_len(&files[a].0)
                    .cmp(&name_len(&files[b].0))
                    .then_with(|| files[a].0.cmp(&files[b].0))
            });
            purged.extend(bucket.iter().skip(1).copied());
        }

        let mut keep_taken: HashSet<String> = HashSet::new();
        let mut purge_taken: HashMap<PathBuf, HashSet<String>> = HashMap::new();

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
                let taken = purge_taken.entry(dest_dir.clone()).or_default();
                let unique = unique_name(&filename, taken);
                taken.insert(unique.clone());
                purged_plans.push(MovePlan {
                    source: path.clone(),
                    target: dest_dir.join(&unique),
                });
            } else if *origin != 0 {
                let unique = unique_name(&filename, &keep_taken);
                keep_taken.insert(unique.clone());
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
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("photos");
    dir.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{name}-purgatory"))
}

fn is_event_name(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() >= 5
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2] == b'-'
        && b[3].is_ascii_digit()
        && b[4].is_ascii_digit()
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

fn collect_image_files(dir: &Path) -> Result<Vec<PathBuf>, FileMoverError> {
    let mut out = Vec::new();
    for entry in WalkDir::new(dir) {
        let entry = entry?;
        if entry.file_type().is_file() && is_supported_image(entry.path()) {
            out.push(entry.into_path());
        }
    }
    Ok(out)
}

fn try_remove_empty(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                try_remove_empty(&p);
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}

#[derive(Debug)]
pub struct CopyOutcome {
    pub copied: usize,
    pub skipped_no_exif: usize,
    pub targets: Vec<PathBuf>,
}

/// Transfer candidate originals that carry valid EXIF data into
/// `destination`, laid out per `format`. Tokens: `YYYY`, `MM`, `DD` come from
/// the EXIF date; `<folder description>` (or `DESC`) comes from the source
/// folder name. `Operation::Copy` leaves the source in place; `Move` removes it.
pub fn copy_originals(
    originals: &[PathBuf],
    destination: &Path,
    format: &str,
) -> Result<CopyOutcome, FileMoverError> {
    copy_originals_with_progress(originals, destination, format, &|_, _| {})
}

pub fn copy_originals_with_progress(
    originals: &[PathBuf],
    destination: &Path,
    format: &str,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<CopyOutcome, FileMoverError> {
    transfer_originals_with_progress(originals, destination, format, Operation::Copy, progress)
}

pub fn transfer_originals_with_progress(
    originals: &[PathBuf],
    destination: &Path,
    format: &str,
    op: Operation,
    progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<CopyOutcome, FileMoverError> {
    let mut outcome = CopyOutcome {
        copied: 0,
        skipped_no_exif: 0,
        targets: Vec::new(),
    };
    let mut taken: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    let total = originals.len();

    for (i, path) in originals.iter().enumerate() {
        match destination_for(path, &creation_date(path), destination, format) {
            Some(target) => {
                let dir = match target.parent() {
                    Some(p) => p.to_path_buf(),
                    None => destination.to_path_buf(),
                };
                let filename = file_name(path);
                let names = taken.entry(dir).or_default();
                let unique = unique_name(&filename, names);
                names.insert(unique.clone());
                let target = target
                    .parent()
                    .unwrap_or_else(|| destination)
                    .join(unique);
                relocate_file(path, &target, op)?;
                outcome.copied += 1;
                outcome.targets.push(target);
            }
            None => outcome.skipped_no_exif += 1,
        }
        progress(i + 1, total);
    }

    Ok(outcome)
}

/// The descriptive part of the folder a file originated from, e.g.
/// `03-15 hawaii` -> `hawaii`; a folder without an `mm-dd` prefix uses its
/// whole name.
fn folder_description(path: &Path) -> String {
    let name = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if is_event_name(name) {
        name[5..].trim().to_string()
    } else {
        name.trim().to_string()
    }
}

/// The full target path an original would be copied to, or `None` when the
/// date isn't a valid calendar date. Pure computation: no file I/O.
pub fn destination_for(
    path: &Path,
    date: &CreationDate,
    destination: &Path,
    format: &str,
) -> Option<PathBuf> {
    let (year, month, day) = parse_date(date)?;
    let description = folder_description(path);
    let rendered = render_date_folder(format, year, month, day, &description);
    Some(destination.join(rendered).join(file_name(path)))
}

pub fn render_date_folder(
    format: &str,
    year: u32,
    month: u32,
    day: u32,
    description: &str,
) -> String {
    let mut out = format.to_string();
    if description.is_empty() {
        out = out
            .replace("<folder description>", "")
            .replace("<desc>", "")
            .replace("DESC", "");
        out = out
            .split('/')
            .map(|seg| {
                seg.trim()
                    .trim_matches(['-', '_', ' '])
                    .trim()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("/");
    } else {
        out = out
            .replace("<folder description>", description)
            .replace("<desc>", description)
            .replace("DESC", description);
    }
    out = out
        .replace("YYYY", &format!("{year:04}"))
        .replace("MM", &format!("{month:02}"))
        .replace("DD", &format!("{day:02}"));
    out
}
