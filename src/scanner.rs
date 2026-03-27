use crate::config::Config;
use crate::db::Db;
use anyhow::Result;
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use parking_lot::Mutex;
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

// ─────────────────────────────────────────────────────────────
// Shared state across all rayon threads during a walk.
// Every field is lock-free or uses a fast mutex.
// ─────────────────────────────────────────────────────────────
struct WalkState {
    min_bytes: u64,
    excludes: Vec<String>,
    results: Mutex<Vec<(String, u64)>>,
    scanned: AtomicU64,
    matched: AtomicU64,
    dirs_entered: AtomicU64,
    interrupted: Arc<AtomicBool>,
    progress: ProgressBar,
    n_threads: usize,
}

#[allow(dead_code)]
pub struct ScanResult {
    pub scan_id: i64,
    pub total_scanned: u64,
    pub files_found: u64,
    pub new_files: u64,
    pub updated_files: u64,
    pub files_per_sec: u64,
    pub duration_secs: f64,
    pub threads: usize,
    pub interrupted: bool,
}

// ─────────────────────────────────────────────────────────────
// TRUE PARALLEL DIRECTORY WALKER
//
// How it works:
//   1. Read a directory's entries with std::fs::read_dir
//   2. Collect entries into a Vec
//   3. Hand the Vec to rayon's par_iter()
//   4. Each entry is processed on ANY available core:
//      - Directories: recurse (spawns more parallel work)
//      - Files: stat() for size, filter, collect
//   5. Rayon's work-stealing scheduler keeps ALL cores busy
//
// Why this is fast:
//   - stat() syscalls (the bottleneck) run on ALL cores in parallel
//   - Directory traversal fans out via rayon's work-stealing deque
//   - No single-threaded consumer loop — every core does real work
//   - Excluded dirs are pruned before recursion (no wasted stat calls)
// ─────────────────────────────────────────────────────────────
fn walk_parallel(dir: &Path, state: &WalkState) {
    if state.interrupted.load(Ordering::Relaxed) {
        return;
    }

    let entries: Vec<fs::DirEntry> = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return, // permission denied, etc.
    };

    state.dirs_entered.fetch_add(1, Ordering::Relaxed);

    // rayon distributes these entries across ALL cores
    entries.par_iter().for_each(|entry| {
        if state.interrupted.load(Ordering::Relaxed) {
            return;
        }

        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => return,
        };

        if ft.is_symlink() {
            return;
        }

        if ft.is_dir() {
            // Check excludes BEFORE recursing — prunes entire subtrees
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if state.excludes.iter().any(|ex| *name_str == **ex) {
                return;
            }
            // Recurse — rayon's work-stealing picks this up on any free core
            walk_parallel(&entry.path(), state);
            return;
        }

        if !ft.is_file() {
            return;
        }

        // ─── File processing (runs on ANY core) ──────────
        let count = state.scanned.fetch_add(1, Ordering::Relaxed);

        // stat() syscall — the expensive part — now truly parallel
        let size = match entry.metadata() {
            Ok(m) => m.len(),
            Err(_) => return,
        };

        if size < state.min_bytes {
            return;
        }

        state.matched.fetch_add(1, Ordering::Relaxed);
        let path_str = entry.path().to_string_lossy().to_string();

        // Push to shared results — lock held < 100ns (just a vec push)
        state.results.lock().push((path_str, size));

        // Update progress ~every 16k files (bitmask = cheap check)
        if count & 0x3FFF == 0 {
            state.progress.set_message(format!(
                "Scanned {} files — {} hits — {} dirs  [{} cores]",
                format_with_commas(count).bold(),
                state
                    .matched
                    .load(Ordering::Relaxed)
                    .to_string()
                    .bold()
                    .red(),
                format_with_commas(state.dirs_entered.load(Ordering::Relaxed)),
                state.n_threads,
            ));
        }
    });
}

// ─────────────────────────────────────────────────────────────
// Public scan entry point
// ─────────────────────────────────────────────────────────────
pub fn scan(config: &Config, config_dir: &Path, db: &Db, resume: bool) -> Result<ScanResult> {
    let min_bytes = config.min_size_bytes()?;
    let n_threads = config.effective_threads();
    let all_scan_paths = config.resolve_scan_paths(config_dir);

    // Build global rayon pool — ALL cores doing real work
    rayon::ThreadPoolBuilder::new()
        .num_threads(n_threads)
        .build_global()
        .ok();

    // ─── Resume logic ───────────────────────────────────────
    let (scan_id, scan_paths) = resolve_scan_paths(db, resume, min_bytes, n_threads, &all_scan_paths)?;

    println!(
        "\n{}  Scanning for files >= {}  (scan #{})",
        "🦈 TREESHARK".bold().cyan(),
        config.min_size.bold().yellow(),
        scan_id.to_string().dimmed(),
    );
    println!(
        "   {}",
        format!(
            "Threshold: {} bytes  •  Threads: {}  •  Paths: {}",
            format_with_commas(min_bytes),
            n_threads,
            scan_paths.len()
        )
        .dimmed()
    );
    println!();

    // ─── Ctrl+C → graceful interrupt ────────────────────────
    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&interrupted);
        ctrlc::set_handler(move || {
            flag.store(true, Ordering::SeqCst);
        })
        .ok();
    }

    let progress = ProgressBar::new_spinner();
    progress.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    progress.enable_steady_tick(std::time::Duration::from_millis(80));

    let start = Instant::now();
    let total_scanned = AtomicU64::new(0);
    let total_matched = AtomicU64::new(0);
    let mut total_new: u64 = 0;
    let mut total_updated: u64 = 0;

    // ─── Walk each path (sequential for resume tracking) ────
    // Within each path, walk_parallel fans out across ALL cores.
    for scan_path in &scan_paths {
        if interrupted.load(Ordering::SeqCst) {
            break;
        }

        progress.set_message(format!(
            "Entering {} ...",
            scan_path.display().to_string().bold()
        ));

        let state = WalkState {
            min_bytes,
            excludes: config.exclude.clone(),
            results: Mutex::new(Vec::with_capacity(4096)),
            scanned: AtomicU64::new(0),
            matched: AtomicU64::new(0),
            dirs_entered: AtomicU64::new(0),
            interrupted: Arc::clone(&interrupted),
            progress: progress.clone(),
            n_threads,
        };

        // ── This is where ALL cores light up ──
        walk_parallel(scan_path, &state);

        // Flush this path's results to SQLite
        let files = std::mem::take(&mut *state.results.lock());
        if !files.is_empty() {
            let (new, updated) = db.upsert_files_batch(&files, scan_id)?;
            total_new += new as u64;
            total_updated += updated as u64;
        }

        // Accumulate totals
        total_scanned.fetch_add(state.scanned.load(Ordering::Relaxed), Ordering::Relaxed);
        total_matched.fetch_add(state.matched.load(Ordering::Relaxed), Ordering::Relaxed);

        // Mark path completed for resume
        if !interrupted.load(Ordering::SeqCst) {
            db.mark_scan_path_completed(scan_id, &scan_path.display().to_string())?;
        }
    }

    let duration = start.elapsed();
    progress.finish_and_clear();

    let scanned = total_scanned.load(Ordering::Relaxed);
    let matched = total_matched.load(Ordering::Relaxed);
    let was_interrupted = interrupted.load(Ordering::SeqCst);

    let files_per_sec = if duration.as_secs_f64() > 0.0 {
        (scanned as f64 / duration.as_secs_f64()) as u64
    } else {
        scanned
    };

    let status = if was_interrupted {
        "interrupted"
    } else {
        "completed"
    };

    db.finish_scan(
        scan_id,
        scanned,
        matched,
        files_per_sec,
        duration.as_secs_f64(),
        status,
    )?;

    // Mark files not seen in this scan as missing — ONLY under the scanned paths,
    // and ONLY files whose size >= this scan's min_size (smaller files weren't looked for)
    if !was_interrupted {
        let scanned_path_strs: Vec<String> = scan_paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let gone = db.mark_missing_from_scan(scan_id, &scanned_path_strs, min_bytes)?;
        if gone > 0 {
            println!(
                "   {} Marked {} files as missing (not found in this scan)",
                "⚠".yellow(),
                gone.to_string().yellow(),
            );
        }
    }

    if was_interrupted {
        println!(
            "   {} Scan interrupted — progress saved. Run {} to continue.",
            "⏸".yellow().bold(),
            "treeshark scan --resume".bold(),
        );
    }
    println!(
        "   {} Scanned {} files in {:.2}s ({} files/sec)",
        "✓".green().bold(),
        format_with_commas(scanned).bold(),
        duration.as_secs_f64(),
        format_with_commas(files_per_sec).bold().green(),
    );
    if total_new > 0 {
        println!(
            "   {} Found {} files >= threshold ({} new, {} already known)",
            "✓".green().bold(),
            matched.to_string().bold().red(),
            total_new.to_string().bold().green(),
            total_updated.to_string().dimmed(),
        );
    } else {
        println!(
            "   {} Found {} files >= threshold (all already known)",
            "✓".green().bold(),
            matched.to_string().bold().red(),
        );
    }
    println!(
        "   {} {} threads saturating {} CPU cores",
        "✓".green().bold(),
        n_threads.to_string().bold().cyan(),
        num_cpus::get().to_string().bold(),
    );
    println!(
        "   {} Stored in {}",
        "✓".green().bold(),
        db.path.display().to_string().dimmed(),
    );
    println!();

    Ok(ScanResult {
        scan_id,
        total_scanned: scanned,
        files_found: matched,
        new_files: total_new,
        updated_files: total_updated,
        files_per_sec,
        duration_secs: duration.as_secs_f64(),
        threads: n_threads,
        interrupted: was_interrupted,
    })
}

// ─── Resume path resolution ─────────────────────────────────
fn resolve_scan_paths(
    db: &Db,
    resume: bool,
    min_bytes: u64,
    n_threads: usize,
    all_scan_paths: &[PathBuf],
) -> Result<(i64, Vec<PathBuf>)> {
    if resume {
        if let Some(prev) = db.get_last_interrupted_scan()? {
            let all_paths: Vec<String> = serde_json::from_str(&prev.scan_paths)?;
            let done: Vec<String> = serde_json::from_str(&prev.completed_paths)?;
            let remaining: Vec<PathBuf> = all_paths
                .iter()
                .filter(|p| !done.contains(p))
                .map(PathBuf::from)
                .collect();

            if remaining.is_empty() {
                println!(
                    "  {} Previous scan #{} already complete. Starting fresh.\n",
                    "→".dimmed(),
                    prev.id
                );
            } else {
                println!(
                    "  {} Resuming scan #{} — {} of {} paths remaining\n",
                    "⏩".bold(),
                    prev.id.to_string().cyan(),
                    remaining.len().to_string().yellow(),
                    all_paths.len(),
                );
                return Ok((prev.id, remaining));
            }
        } else {
            println!(
                "  {} No interrupted scan found. Starting fresh.\n",
                "→".dimmed()
            );
        }
    }

    let paths_str: Vec<String> = all_scan_paths.iter().map(|p| p.display().to_string()).collect();
    let id = db.create_scan(min_bytes, &paths_str, n_threads as u32)?;
    Ok((id, all_scan_paths.to_vec()))
}

fn format_with_commas(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}
