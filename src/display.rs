use crate::db::{Db, DbStats, FileRow, ScanRow};
use anyhow::Result;
use bytesize::ByteSize;
use colored::*;

pub fn print_files(files: &[FileRow], title: &str) {
    println!(
        "\n{}",
        "┌──────────────────────────────────────────────────────────────────────────────┐"
            .cyan()
            .bold()
    );
    println!(
        "{}  🦈 TREESHARK — {}",
        "│".cyan().bold(),
        title.bold(),
    );
    println!(
        "{}",
        "└──────────────────────────────────────────────────────────────────────────────┘"
            .cyan()
            .bold()
    );

    if files.is_empty() {
        println!(
            "\n  {} No files found. Your disk is clean! 🎉\n",
            "→".green()
        );
        return;
    }

    println!();
    println!(
        "  {}   {:>10}   {:>8}   {:>5}   {}",
        "#".dimmed(),
        "SIZE".dimmed().bold(),
        "STATUS".dimmed().bold(),
        "SEEN".dimmed().bold(),
        "PATH".dimmed().bold(),
    );
    println!("  {}", "─".repeat(90).dimmed());

    let mut total_size: u64 = 0;

    for (i, file) in files.iter().enumerate() {
        let size_str = format_size_colored(file.size);
        let idx = format!("{:>3}", i + 1);
        let status = file.status.colored();
        let seen = format!("{:>3}x", file.times_seen);

        let path = &file.path;
        let (dir, name) = match path.rfind('/') {
            Some(pos) => (&path[..=pos], &path[pos + 1..]),
            None => ("", path.as_str()),
        };

        println!(
            "  {}   {:>10}   {:>8}   {}   {}{}",
            idx.dimmed(),
            size_str,
            status,
            seen.dimmed(),
            dir.dimmed(),
            name.white().bold()
        );

        total_size += file.size;
    }

    println!("  {}", "─".repeat(90).dimmed());
    println!(
        "  {}  {:>10}   {} files",
        "Σ".bold().cyan(),
        ByteSize(total_size).to_string().red().bold(),
        files.len().to_string().bold()
    );
    println!();
}

pub fn print_history(scans: &[ScanRow]) {
    println!(
        "\n{}",
        "┌──────────────────────────────────────────────────────────────────────────────┐"
            .cyan()
            .bold()
    );
    println!(
        "{}  🦈 TREESHARK — Scan History",
        "│".cyan().bold(),
    );
    println!(
        "{}",
        "└──────────────────────────────────────────────────────────────────────────────┘"
            .cyan()
            .bold()
    );

    if scans.is_empty() {
        println!("\n  {} No scans recorded yet.\n", "→".dimmed());
        return;
    }

    println!();
    println!(
        "  {:>4}   {:>19}   {:>10}   {:>10}   {:>8}   {:>10}   {}",
        "ID".dimmed().bold(),
        "DATE".dimmed().bold(),
        "SCANNED".dimmed().bold(),
        "FOUND".dimmed().bold(),
        "TIME".dimmed().bold(),
        "SPEED".dimmed().bold(),
        "STATUS".dimmed().bold(),
    );
    println!("  {}", "─".repeat(90).dimmed());

    for scan in scans {
        let date = if scan.started_at.len() >= 19 {
            &scan.started_at[..19]
        } else {
            &scan.started_at
        };

        let status_colored = match scan.status.as_str() {
            "completed" => "completed".green().to_string(),
            "interrupted" => "interrupted".yellow().to_string(),
            "running" => "running".cyan().to_string(),
            other => other.to_string(),
        };

        println!(
            "  {:>4}   {:>19}   {:>10}   {:>10}   {:>7.1}s   {:>8}/s   {}",
            format!("#{}", scan.id).cyan(),
            date.dimmed(),
            format_with_commas(scan.total_scanned),
            scan.files_found.to_string().red(),
            scan.duration_secs,
            format_with_commas(scan.files_per_sec).green(),
            status_colored,
        );
    }
    println!();
}

pub fn print_stats(stats: &DbStats, db: &Db) -> Result<()> {
    println!(
        "\n{}",
        "┌──────────────────────────────────────────────────────────────────────────────┐"
            .cyan()
            .bold()
    );
    println!(
        "{}  🦈 TREESHARK — Database Stats",
        "│".cyan().bold(),
    );
    println!(
        "{}",
        "└──────────────────────────────────────────────────────────────────────────────┘"
            .cyan()
            .bold()
    );
    println!();
    println!(
        "  {}   {}",
        "Database:".bold(),
        db.path.display().to_string().dimmed()
    );
    println!(
        "  {}   {}",
        "Total scans:".bold(),
        stats.total_scans.to_string().cyan()
    );
    println!();
    println!(
        "  {}   {:>6}   (all statuses)",
        "Files tracked:".bold(),
        stats.total_files.to_string().white().bold(),
    );
    println!(
        "    {}   {:>6}   {}",
        "├─ exists:".bold(),
        stats.exists.to_string().green(),
        ByteSize(stats.total_size_exists).to_string().green()
    );
    println!(
        "    {}   {:>6}   {} freed",
        "├─ deleted:".bold(),
        stats.deleted.to_string().red(),
        ByteSize(stats.total_size_deleted).to_string().red()
    );
    println!(
        "    {}   {:>6}",
        "└─ missing:".bold(),
        stats.missing.to_string().yellow(),
    );
    println!();
    Ok(())
}

fn format_size_colored(size: u64) -> String {
    let bs = ByteSize(size);
    let s = bs.to_string();
    if size >= 1 << 40 {
        s.red().bold().to_string()
    } else if size >= 1 << 30 {
        s.red().to_string()
    } else if size >= 100 * (1 << 20) {
        s.yellow().to_string()
    } else {
        s.white().to_string()
    }
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
