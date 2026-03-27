use crate::db::Db;
use anyhow::{Context, Result};
use bytesize::ByteSize;
use colored::*;
use dialoguer::{theme::ColorfulTheme, Confirm, MultiSelect};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};

fn build_theme() -> ColorfulTheme {
    ColorfulTheme {
        prompt_prefix: dialoguer::console::style("🦈".to_string()).cyan().bold(),
        prompt_style: dialoguer::console::Style::new().bold(),
        ..ColorfulTheme::default()
    }
}

/// Filter helper used by both move and restore
pub fn filter_by_ext(
    path: &str,
    filter_exts: &[String],
    exclude_exts: &[String],
) -> bool {
    let path_lower = path.to_lowercase();
    if !filter_exts.is_empty()
        && !filter_exts
            .iter()
            .any(|ext| path_lower.ends_with(&format!(".{}", ext)))
    {
        return false;
    }
    if !exclude_exts.is_empty()
        && exclude_exts
            .iter()
            .any(|ext| path_lower.ends_with(&format!(".{}", ext)))
    {
        return false;
    }
    true
}

/// Build a human-readable filter label
fn filter_label(filter_exts: &[String], exclude_exts: &[String]) -> String {
    let mut parts = Vec::new();
    if !filter_exts.is_empty() {
        let ext_list: Vec<String> = filter_exts.iter().map(|e| format!(".{}", e)).collect();
        parts.push(format!("only: {}", ext_list.join(", ")));
    }
    if !exclude_exts.is_empty() {
        let ext_list: Vec<String> = exclude_exts.iter().map(|e| format!(".{}", e)).collect();
        parts.push(format!("excluding: {}", ext_list.join(", ")));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join(" / "))
    }
}

// ─────────────────────────────────────────────────────────────
// MOVE — stage files into a managed review folder
// ─────────────────────────────────────────────────────────────
/// Build the destination path for a file.
/// - Normal mode: preserves full directory structure under dest (dest/<full_path>)
/// - Full-organized mode: groups by extension (dest/__<ext>__/<filename>)
fn build_dest_path(file_path: &str, dest_abs: &Path, full_organized: bool) -> PathBuf {
    if full_organized {
        let p = Path::new(file_path);
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "noext".to_string());
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let filename = p
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let mut dst = dest_abs.join(format!("__{}__", ext)).join(&filename);
        // Handle collisions: append _1, _2, ... if file already exists
        let mut counter = 1u32;
        while dst.exists() {
            let new_name = format!("{}_{}.{}", stem, counter, ext);
            dst = dest_abs.join(format!("__{}__", ext)).join(new_name);
            counter += 1;
        }
        dst
    } else {
        let rel = file_path.strip_prefix('/').unwrap_or(file_path);
        dest_abs.join(rel)
    }
}

pub fn move_files(
    db: &Db,
    top_n: usize,
    path_prefixes: &[String],
    filter_exts: &[String],
    exclude_exts: &[String],
    dest: &Path,
    full_organized: bool,
) -> Result<()> {
    let all_files = db.get_top_files(top_n, Some("exists"), path_prefixes)?;

    let files: Vec<_> = all_files
        .into_iter()
        .filter(|f| filter_by_ext(&f.path, filter_exts, exclude_exts))
        .collect();

    if files.is_empty() {
        println!(
            "\n  {} No files matching filters{}\n",
            "⚠".yellow(),
            filter_label(filter_exts, exclude_exts).yellow(),
        );
        return Ok(());
    }

    let total_size: u64 = files.iter().map(|f| f.size).sum();
    let dest_abs = if dest.is_absolute() {
        dest.to_path_buf()
    } else {
        std::env::current_dir()?.join(dest)
    };

    let mode_label = if full_organized {
        " [full-organized: __<ext>__/ folders]"
    } else {
        ""
    };
    println!(
        "\n{}  Move {} files ({}) → {}{}{}\n",
        "🦈 TREESHARK".bold().cyan(),
        files.len().to_string().bold(),
        ByteSize(total_size).to_string().yellow(),
        dest_abs.display().to_string().bold(),
        mode_label.cyan(),
        filter_label(filter_exts, exclude_exts).dimmed(),
    );

    // Preview files
    let show_n = std::cmp::min(files.len(), 20);
    for f in &files[..show_n] {
        println!(
            "    {} {:>8}  {}",
            "→".dimmed(),
            ByteSize(f.size).to_string().dimmed(),
            f.path
        );
    }
    if files.len() > show_n {
        println!(
            "    {} ... and {} more files",
            "→".dimmed(),
            (files.len() - show_n).to_string().yellow(),
        );
    }
    println!();

    // ── Pre-flight permission check ──────────────────────────
    let problem_files: Vec<&crate::db::FileRow> = files
        .iter()
        .filter(|f| {
            let src = Path::new(&f.path);
            // Check: file readable + parent dir writable (needed for rename/remove)
            if !src.exists() {
                return false; // will be skipped anyway
            }
            let readable = std::fs::File::open(src).is_ok();
            let parent_writable = src
                .parent()
                .map(|p| {
                    let test = p.join(".treeshark_write_test");
                    if std::fs::File::create(&test).is_ok() {
                        std::fs::remove_file(&test).ok();
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            !readable || !parent_writable
        })
        .collect();

    if !problem_files.is_empty() {
        let problem_size: u64 = problem_files.iter().map(|f| f.size).sum();
        println!(
            "  {} {} files ({}) have permission issues:\n",
            "⚠".red().bold(),
            problem_files.len().to_string().red().bold(),
            ByteSize(problem_size).to_string().red(),
        );
        let show_problems = std::cmp::min(problem_files.len(), 15);
        for f in &problem_files[..show_problems] {
            println!(
                "    {} {:>8}  {}",
                "✗".red(),
                ByteSize(f.size).to_string().dimmed(),
                f.path.red(),
            );
        }
        if problem_files.len() > show_problems {
            println!(
                "    {} ... and {} more",
                "✗".red(),
                (problem_files.len() - show_problems).to_string().red(),
            );
        }
        println!();

        let theme = build_theme();
        let choices = &[
            "Abort — fix permissions first, then retry",
            "Skip — move only the files that work",
            "Try fix — chmod u+rw files & parent dirs, then move all",
        ];
        let selection = dialoguer::Select::with_theme(&theme)
            .with_prompt(format!(
                "{} files have permission problems",
                problem_files.len()
            ))
            .items(choices)
            .default(0)
            .interact()
            .context("Selection cancelled")?;

        match selection {
            0 => {
                // Abort
                println!("\n  {} Aborted. Fix permissions and retry.\n", "→".dimmed());
                println!(
                    "  {} Example: {}",
                    "💡".bold(),
                    "sudo chmod -R u+rw /path/to/dir".dimmed(),
                );
                println!();
                return Ok(());
            }
            2 => {
                // Try fix permissions
                println!();
                let mut fixed = 0u32;
                let mut fix_failed = 0u32;
                for f in &problem_files {
                    let src = Path::new(&f.path);
                    // chmod u+rw on the file
                    let file_ok = std::process::Command::new("chmod")
                        .args(["u+rw", &f.path])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    // chmod u+rwx on the parent dir
                    let parent_ok = src
                        .parent()
                        .map(|p| {
                            std::process::Command::new("chmod")
                                .args(["u+rwx", &p.to_string_lossy()])
                                .status()
                                .map(|s| s.success())
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);

                    if file_ok && parent_ok {
                        fixed += 1;
                    } else {
                        fix_failed += 1;
                        eprintln!(
                            "    {} chmod failed: {}",
                            "✗".red(),
                            f.path.dimmed(),
                        );
                    }
                }
                println!(
                    "  {} Fixed permissions on {} files",
                    "✓".green(),
                    fixed.to_string().green().bold(),
                );
                if fix_failed > 0 {
                    println!(
                        "  {} {} files still have issues (will be skipped)",
                        "⚠".yellow(),
                        fix_failed.to_string().yellow(),
                    );
                }
                println!();
            }
            _ => {
                // Skip — just continue, the move loop already handles errors
                println!(
                    "\n  {} Skipping {} problem files, moving the rest.\n",
                    "→".dimmed(),
                    problem_files.len(),
                );
            }
        }
    }

    let theme = build_theme();
    let confirm = Confirm::with_theme(&theme)
        .with_prompt(format!(
            "Move {} files ({}) to staging folder?",
            files.len(),
            ByteSize(total_size)
        ))
        .default(false)
        .interact()
        .context("Confirmation cancelled")?;

    if !confirm {
        println!("\n  {} Aborted. No files were moved.\n", "→".dimmed());
        return Ok(());
    }

    // Progress bar
    let pb = ProgressBar::new(files.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "   {spinner:.cyan} [{bar:30.cyan/dim}] {pos}/{len}  {msg}",
        )
        .unwrap()
        .progress_chars("█▓░"),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(80));

    let mut moved_count: u64 = 0;
    let mut moved_size: u64 = 0;
    let mut failed_count: u64 = 0;

    for file in &files {
        let src = Path::new(&file.path);

        // Build destination path (full-organized or preserving structure)
        let dst = build_dest_path(&file.path, &dest_abs, full_organized);

        pb.set_message(truncate_path(&file.path, 60));

        // Create parent dirs
        if let Some(parent) = dst.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                pb.suspend(|| {
                    eprintln!(
                        "    {} mkdir failed: {} — {}",
                        "✗".red(),
                        parent.display(),
                        e
                    );
                });
                failed_count += 1;
                pb.inc(1);
                continue;
            }
        }

        // Try rename first (instant on same filesystem)
        match std::fs::rename(src, &dst) {
            Ok(()) => {
                db.mark_moved(&file.path, &dst.to_string_lossy())?;
                moved_count += 1;
                moved_size += file.size;
            }
            Err(e) => {
                // Cross-device (EXDEV = 18): fall back to copy + delete
                if e.raw_os_error() == Some(18) {
                    pb.set_message(format!(
                        "copying {} ({})",
                        truncate_path(&file.path, 40),
                        ByteSize(file.size)
                    ));
                    match copy_and_remove(src, &dst) {
                        Ok(()) => {
                            db.mark_moved(&file.path, &dst.to_string_lossy())?;
                            moved_count += 1;
                            moved_size += file.size;
                        }
                        Err(e2) => {
                            pb.suspend(|| {
                                eprintln!(
                                    "    {} copy failed: {} — {}",
                                    "✗".red(),
                                    file.path,
                                    e2
                                );
                            });
                            failed_count += 1;
                        }
                    }
                } else {
                    pb.suspend(|| {
                        eprintln!("    {} move failed: {} — {}", "✗".red(), file.path, e);
                    });
                    failed_count += 1;
                }
            }
        }
        pb.inc(1);
    }

    pb.finish_and_clear();

    println!(
        "\n  {} Moved {} files ({})",
        "🦈".bold(),
        moved_count.to_string().green().bold(),
        ByteSize(moved_size).to_string().green().bold(),
    );
    if failed_count > 0 {
        println!(
            "  {} {} files failed to move",
            "⚠".yellow(),
            failed_count.to_string().red(),
        );
    }
    println!(
        "  {} Staged in {}",
        "📁".bold(),
        dest_abs.display().to_string().bold()
    );
    println!(
        "  {} Review files there, then delete what you don't need.",
        "→".dimmed()
    );
    println!(
        "  {} Use {} to put files back.\n",
        "→".dimmed(),
        "treeshark restore".bold()
    );

    Ok(())
}

// ─────────────────────────────────────────────────────────────
// RESTORE — move files back from staging to original location
// ─────────────────────────────────────────────────────────────
pub fn restore_files(
    db: &Db,
    filter_exts: &[String],
    exclude_exts: &[String],
) -> Result<()> {
    let all_moved = db.get_moved_files()?;

    let files: Vec<_> = all_moved
        .into_iter()
        .filter(|f| filter_by_ext(&f.path, filter_exts, exclude_exts))
        .collect();

    if files.is_empty() {
        println!(
            "\n  {} No moved files to restore{}\n",
            "⚠".yellow(),
            filter_label(filter_exts, exclude_exts).yellow(),
        );
        return Ok(());
    }

    let total_size: u64 = files.iter().map(|f| f.size).sum();

    println!(
        "\n{}  Restore {} files ({}) back to original locations{}\n",
        "🦈 TREESHARK".bold().cyan(),
        files.len().to_string().bold(),
        ByteSize(total_size).to_string().yellow(),
        filter_label(filter_exts, exclude_exts).dimmed(),
    );

    let show_n = std::cmp::min(files.len(), 20);
    for f in &files[..show_n] {
        let from = f.moved_to.as_deref().unwrap_or("?");
        println!(
            "    {} {:>8}  {} → {}",
            "←".dimmed(),
            ByteSize(f.size).to_string().dimmed(),
            truncate_path(from, 30).dimmed(),
            f.path,
        );
    }
    if files.len() > show_n {
        println!(
            "    {} ... and {} more files",
            "←".dimmed(),
            (files.len() - show_n).to_string().yellow(),
        );
    }
    println!();

    let theme = build_theme();
    let confirm = Confirm::with_theme(&theme)
        .with_prompt(format!("Restore {} files to original locations?", files.len()))
        .default(false)
        .interact()
        .context("Confirmation cancelled")?;

    if !confirm {
        println!("\n  {} Aborted.\n", "→".dimmed());
        return Ok(());
    }

    let pb = ProgressBar::new(files.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "   {spinner:.cyan} [{bar:30.cyan/dim}] {pos}/{len}  {msg}",
        )
        .unwrap()
        .progress_chars("█▓░"),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(80));

    let mut restored_count: u64 = 0;
    let mut failed_count: u64 = 0;

    for file in &files {
        let moved_to = match &file.moved_to {
            Some(p) => p.clone(),
            None => {
                pb.suspend(|| {
                    eprintln!("    {} No moved_to path for: {}", "✗".red(), file.path);
                });
                failed_count += 1;
                pb.inc(1);
                continue;
            }
        };

        let src = Path::new(&moved_to);
        let dst = Path::new(&file.path);

        pb.set_message(truncate_path(&file.path, 60));

        if !src.exists() {
            pb.suspend(|| {
                eprintln!(
                    "    {} Staged file missing: {}",
                    "✗".red(),
                    moved_to.dimmed()
                );
            });
            failed_count += 1;
            pb.inc(1);
            continue;
        }

        // Ensure original parent dir exists
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        match std::fs::rename(src, dst) {
            Ok(()) => {
                db.mark_restored(&file.path)?;
                restored_count += 1;
            }
            Err(e) => {
                if e.raw_os_error() == Some(18) {
                    match copy_and_remove(src, dst) {
                        Ok(()) => {
                            db.mark_restored(&file.path)?;
                            restored_count += 1;
                        }
                        Err(e2) => {
                            pb.suspend(|| {
                                eprintln!(
                                    "    {} restore failed: {} — {}",
                                    "✗".red(),
                                    file.path,
                                    e2
                                );
                            });
                            failed_count += 1;
                        }
                    }
                } else {
                    pb.suspend(|| {
                        eprintln!("    {} restore failed: {} — {}", "✗".red(), file.path, e);
                    });
                    failed_count += 1;
                }
            }
        }
        pb.inc(1);
    }

    pb.finish_and_clear();

    println!(
        "\n  {} Restored {} files to original locations",
        "🦈".bold(),
        restored_count.to_string().green().bold(),
    );
    if failed_count > 0 {
        println!(
            "  {} {} files failed to restore",
            "⚠".yellow(),
            failed_count.to_string().red(),
        );
    }
    println!("  {} Database updated.\n", "✓".green());

    // Cleanup empty dirs in staging
    cleanup_empty_dirs_hint(&files);

    Ok(())
}

// ─── Helpers ─────────────────────────────────────────────────

/// Copy file then remove original — fallback for cross-device moves
fn copy_and_remove(src: &Path, dst: &Path) -> Result<()> {
    std::fs::copy(src, dst).with_context(|| format!("copy {} → {}", src.display(), dst.display()))?;
    std::fs::remove_file(src)
        .with_context(|| format!("remove source after copy: {}", src.display()))?;
    Ok(())
}

/// Truncate a path string for display (UTF-8 safe)
fn truncate_path(path: &str, max_len: usize) -> String {
    if path.chars().count() <= max_len {
        return path.to_string();
    }
    let total = path.chars().count();
    let skip = total - (max_len.saturating_sub(3));
    let suffix: String = path.chars().skip(skip).collect();
    format!("...{}", suffix)
}

/// Hint about cleaning up empty staging dirs after restore
fn cleanup_empty_dirs_hint(files: &[crate::db::FileRow]) {
    // Collect unique staging root dirs
    let roots: std::collections::HashSet<PathBuf> = files
        .iter()
        .filter_map(|f| {
            f.moved_to.as_ref().and_then(|p| {
                // Find the staging root (first component after CWD)
                let pb = PathBuf::from(p);
                pb.ancestors()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .nth(1)
                    .map(|a| a.to_path_buf())
            })
        })
        .collect();

    if !roots.is_empty() {
        for root in &roots {
            if root.exists() {
                println!(
                    "  {} To clean up empty staging dirs: {}",
                    "💡".bold(),
                    format!("find {} -type d -empty -delete", root.display()).dimmed(),
                );
            }
        }
        println!();
    }
}

// ─────────────────────────────────────────────────────────────
// PURGE — interactively delete __<ext>__ folders from stock
// ─────────────────────────────────────────────────────────────

/// Info about one __<ext>__ folder
struct ExtFolder {
    ext: String,
    path: PathBuf,
    file_count: u64,
    total_size: u64,
}

/// Scan a directory for __<ext>__ folders and collect stats
fn scan_ext_folders(stock_dir: &Path) -> Result<Vec<ExtFolder>> {
    let mut folders = Vec::new();

    let entries = std::fs::read_dir(stock_dir)
        .with_context(|| format!("Cannot read directory: {}", stock_dir.display()))?;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("__") || !name.ends_with("__") || name.len() < 5 {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let ext = name[2..name.len() - 2].to_string();
        let mut file_count: u64 = 0;
        let mut total_size: u64 = 0;

        // Walk recursively to get accurate count/size
        fn walk_dir(dir: &Path, count: &mut u64, size: &mut u64) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        walk_dir(&p, count, size);
                    } else if let Ok(meta) = p.metadata() {
                        *count += 1;
                        *size += meta.len();
                    }
                }
            }
        }
        walk_dir(&path, &mut file_count, &mut total_size);

        folders.push(ExtFolder {
            ext,
            path,
            file_count,
            total_size,
        });
    }

    // Sort by size descending
    folders.sort_by(|a, b| b.total_size.cmp(&a.total_size));
    Ok(folders)
}

pub fn purge_ext_folders(db: &Db, stock_dir: &Path) -> Result<()> {
    if !stock_dir.exists() {
        println!(
            "\n  {} Stock directory not found: {}\n",
            "⚠".yellow(),
            stock_dir.display(),
        );
        return Ok(());
    }

    let folders = scan_ext_folders(stock_dir)?;

    if folders.is_empty() {
        println!(
            "\n  {} No __<ext>__ folders found in {}\n",
            "⚠".yellow(),
            stock_dir.display(),
        );
        return Ok(());
    }

    let grand_total_size: u64 = folders.iter().map(|f| f.total_size).sum();
    let grand_total_files: u64 = folders.iter().map(|f| f.file_count).sum();

    println!(
        "\n{}  {} extension folders in {} ({} files, {})\n",
        "🦈 TREESHARK".bold().cyan(),
        folders.len().to_string().bold(),
        stock_dir.display().to_string().bold(),
        grand_total_files.to_string().yellow(),
        ByteSize(grand_total_size).to_string().yellow(),
    );

    // Build picker items
    let items: Vec<String> = folders
        .iter()
        .enumerate()
        .map(|(i, f)| {
            format!(
                "{:>3}. {:>8}]  __{:}__  ({} files)",
                i + 1,
                ByteSize(f.total_size),
                f.ext,
                f.file_count,
            )
        })
        .collect();

    let theme = ColorfulTheme {
        active_item_style: dialoguer::console::Style::new()
            .cyan()
            .bold()
            .on_color256(236),
        active_item_prefix: dialoguer::console::style("  ▸ [".to_string())
            .cyan()
            .bold(),
        inactive_item_prefix: dialoguer::console::style("    [".to_string()).dim(),
        checked_item_prefix: dialoguer::console::style("  ✓ [".to_string())
            .red()
            .bold(),
        unchecked_item_prefix: dialoguer::console::style("    [".to_string()).dim(),
        prompt_prefix: dialoguer::console::style("🦈".to_string()).cyan().bold(),
        prompt_style: dialoguer::console::Style::new().bold(),
        ..ColorfulTheme::default()
    };

    let selections = MultiSelect::with_theme(&theme)
        .with_prompt("Pick extension folders to DELETE (↑↓ navigate, space toggle, enter confirm)")
        .items(&items)
        .interact()
        .context("Selection cancelled")?;

    if selections.is_empty() {
        println!("\n  {} Nothing selected.\n", "→".dimmed());
        return Ok(());
    }

    // Show summary of what will be nuked
    let mut nuke_size: u64 = 0;
    let mut nuke_files: u64 = 0;
    println!(
        "\n  {} Folders marked for deletion:\n",
        "⚠".red().bold(),
    );
    for &idx in &selections {
        let f = &folders[idx];
        nuke_size += f.total_size;
        nuke_files += f.file_count;
        println!(
            "    {} {:>8}  __{}__/  ({} files)",
            "✗".red(),
            ByteSize(f.total_size).to_string().red(),
            f.ext.red().bold(),
            f.file_count,
        );
    }
    println!(
        "\n    {} Total: {} in {} files",
        "Σ".bold(),
        ByteSize(nuke_size).to_string().red().bold(),
        nuke_files.to_string().red().bold(),
    );
    println!();

    let confirm = Confirm::with_theme(&theme)
        .with_prompt(format!(
            "Permanently delete {} folders ({}, {} files)? This cannot be undone",
            selections.len(),
            ByteSize(nuke_size),
            nuke_files,
        ))
        .default(false)
        .interact()
        .context("Confirmation cancelled")?;

    if !confirm {
        println!("\n  {} Aborted.\n", "→".dimmed());
        return Ok(());
    }

    // Delete folders with progress
    let pb = ProgressBar::new(selections.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "   {spinner:.cyan} [{bar:30.cyan/dim}] {pos}/{len}  {msg}",
        )
        .unwrap()
        .progress_chars("█▓░"),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(80));

    let mut deleted_folders: u64 = 0;
    let mut freed: u64 = 0;
    let mut failed: u64 = 0;

    for &idx in &selections {
        let f = &folders[idx];
        pb.set_message(format!("__{}__", f.ext));

        // Mark all files in this folder as deleted in db
        // Walk the folder and match against db paths
        mark_folder_deleted_in_db(db, &f.path);

        match std::fs::remove_dir_all(&f.path) {
            Ok(()) => {
                deleted_folders += 1;
                freed += f.total_size;
            }
            Err(e) => {
                pb.suspend(|| {
                    eprintln!(
                        "    {} Failed to delete __{}__: {}",
                        "✗".red(),
                        f.ext,
                        e,
                    );
                });
                failed += 1;
            }
        }
        pb.inc(1);
    }

    pb.finish_and_clear();

    println!(
        "\n  {} Deleted {} extension folders, freed {}",
        "🦈".bold(),
        deleted_folders.to_string().green().bold(),
        ByteSize(freed).to_string().green().bold(),
    );
    if failed > 0 {
        println!(
            "  {} {} folders failed to delete",
            "⚠".yellow(),
            failed.to_string().red(),
        );
    }
    println!("  {} Database updated.\n", "✓".green());

    Ok(())
}

/// Walk an __<ext>__ folder and mark each file as deleted in the db.
/// Files in full-organized folders have moved_to pointing here, so we
/// look up by moved_to and mark the original path as deleted.
fn mark_folder_deleted_in_db(db: &Db, folder: &Path) {
    fn walk(db: &Db, dir: &Path) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    walk(db, &p);
                } else {
                    let moved_path = p.to_string_lossy().to_string();
                    // Try to mark by moved_to (full-organized files)
                    let _ = db.mark_deleted_by_moved_to(&moved_path);
                }
            }
        }
    }
    walk(db, folder);
}
