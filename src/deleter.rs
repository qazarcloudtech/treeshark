use crate::db::Db;
use anyhow::{Context, Result};
use bytesize::ByteSize;
use colored::*;
use dialoguer::{theme::ColorfulTheme, Confirm, MultiSelect};
use std::path::Path;

fn build_theme() -> ColorfulTheme {
    ColorfulTheme {
        active_item_style: dialoguer::console::Style::new()
            .cyan()
            .bold()
            .on_color256(236), // bright text on dark gray background
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
    }
}

pub fn interactive_delete(db: &Db, top_n: usize, path_prefixes: &[String]) -> Result<()> {
    let files = db.get_top_files(top_n, Some("exists"), path_prefixes)?;

    if files.is_empty() {
        if path_prefixes.is_empty() {
            println!(
                "\n  {} No existing files in database. Run {} first.\n",
                "⚠".yellow(),
                "treeshark scan".bold()
            );
        } else {
            println!(
                "\n  {} No existing files under scanned paths:",
                "⚠".yellow(),
            );
            for p in path_prefixes {
                println!("      {}", p.dimmed());
            }
            println!(
                "\n  Run {} or use {} to see all paths.\n",
                "treeshark scan".bold(),
                "treeshark delete --all".bold(),
            );
        }
        return Ok(());
    }

    println!(
        "\n{}  Select files to {} (↑↓ navigate, space toggle, enter confirm)\n",
        "🦈 TREESHARK".bold().cyan(),
        "DELETE".red().bold()
    );

    let items: Vec<String> = files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let size = ByteSize(f.size);
            let (dir, name) = match f.path.rfind('/') {
                Some(pos) => (&f.path[..=pos], &f.path[pos + 1..]),
                None => ("", f.path.as_str()),
            };
            format!("{:>3}. {:>8}]  {}{}", i + 1, size, dir, name)
        })
        .collect();

    let theme = build_theme();
    let selections = MultiSelect::with_theme(&theme)
        .with_prompt("Pick files to nuke 💀")
        .items(&items)
        .interact()
        .context("Selection cancelled")?;

    if selections.is_empty() {
        println!("\n  {} No files selected. Nothing to do.\n", "→".dimmed());
        return Ok(());
    }

    // Show what will be deleted
    println!("\n  {} Files marked for deletion:\n", "⚠".yellow().bold());
    let mut total_freed: u64 = 0;
    for &idx in &selections {
        let f = &files[idx];
        total_freed += f.size;
        println!(
            "    {} {:>8}  {}",
            "✗".red(),
            ByteSize(f.size).to_string().red(),
            f.path
        );
    }
    println!(
        "\n    {} Total to free: {}",
        "Σ".bold(),
        ByteSize(total_freed).to_string().red().bold()
    );
    println!();

    let confirm = Confirm::with_theme(&theme)
        .with_prompt(format!(
            "Permanently delete {} files ({})? This cannot be undone",
            selections.len(),
            ByteSize(total_freed)
        ))
        .default(false)
        .interact()
        .context("Confirmation cancelled")?;

    if !confirm {
        println!("\n  {} Aborted. No files were deleted.\n", "→".dimmed());
        return Ok(());
    }

    let mut deleted_count = 0;
    let mut freed: u64 = 0;
    let mut failed_count = 0;

    for &idx in &selections {
        let file = &files[idx];
        let path = Path::new(&file.path);

        match std::fs::remove_file(path) {
            Ok(()) => {
                db.mark_deleted(&file.path)?;
                println!("    {} Deleted: {}", "✓".green(), file.path.dimmed());
                freed += file.size;
                deleted_count += 1;
            }
            Err(e) => {
                if !path.exists() {
                    db.mark_deleted(&file.path)?;
                    println!(
                        "    {} Already gone: {}",
                        "~".yellow(),
                        file.path.dimmed()
                    );
                } else {
                    println!(
                        "    {} Failed: {} — {}",
                        "✗".red(),
                        file.path,
                        e.to_string().red()
                    );
                    failed_count += 1;
                }
            }
        }
    }

    println!();
    println!(
        "  {} Deleted {} files, freed {}",
        "🦈".bold(),
        deleted_count.to_string().green().bold(),
        ByteSize(freed).to_string().green().bold()
    );
    if failed_count > 0 {
        println!(
            "  {} {} files failed to delete",
            "⚠".yellow(),
            failed_count.to_string().red()
        );
    }
    println!("  {} Database updated.\n", "✓".green());

    Ok(())
}
