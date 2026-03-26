mod config;
mod db;
mod deleter;
mod display;
mod scanner;

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use config::Config;
use db::Db;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "treeshark",
    about = "🦈 Hunt down the biggest files devouring your disk — blazingly fast",
    version,
    styles = get_styles()
)]
struct Cli {
    /// Path to config YAML file
    #[arg(short, long, default_value = "config.yml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 🔍 Scan directories for big files (parallel, all cores)
    Scan {
        /// Override min size from config (e.g., "500MB", "2GB")
        #[arg(short, long)]
        min_size: Option<String>,

        /// Override scan path (can be repeated)
        #[arg(short, long)]
        path: Vec<String>,

        /// Number of threads (0 = all cores)
        #[arg(short, long)]
        threads: Option<usize>,

        /// Resume an interrupted scan
        #[arg(short, long)]
        resume: bool,
    },

    /// 📋 List biggest files from database (scoped to config scan_paths)
    List {
        /// Show top N files
        #[arg(short = 'n', long, default_value = "50")]
        top: usize,

        /// Filter by status: exists, deleted, missing, all
        #[arg(short, long, default_value = "exists")]
        status: String,

        /// Show files from ALL paths (ignore scan_paths scope)
        #[arg(short, long)]
        all: bool,

        /// Filter to a specific path (can be repeated)
        #[arg(short = 'P', long = "path")]
        filter_path: Vec<String>,
    },

    /// 🗑️  Interactively select and delete files (scoped to config scan_paths)
    Delete {
        /// Show files from ALL paths (ignore scan_paths scope)
        #[arg(short, long)]
        all: bool,

        /// Filter to a specific path (can be repeated)
        #[arg(short = 'P', long = "path")]
        filter_path: Vec<String>,
    },

    /// 📊 Show database stats
    Stats,

    /// 📜 Show scan history
    History {
        /// Number of past scans to show
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
    },

    /// 🧹 Reset the database (delete all data)
    Reset,

    /// ⚙️  Show current config
    Config,
}

fn get_styles() -> clap::builder::Styles {
    clap::builder::Styles::styled()
        .header(
            clap::builder::styling::AnsiColor::Cyan
                .on_default()
                .bold(),
        )
        .usage(
            clap::builder::styling::AnsiColor::Cyan
                .on_default()
                .bold(),
        )
        .literal(
            clap::builder::styling::AnsiColor::Green
                .on_default()
                .bold(),
        )
        .placeholder(clap::builder::styling::AnsiColor::Yellow.on_default())
}

/// Resolve path prefixes for scoping list/delete queries.
/// Priority: --path flags > config scan_paths (unless --all)
fn resolve_path_filter(
    all: bool,
    cli_paths: &[String],
    config: &Config,
    config_dir: &Path,
) -> Vec<String> {
    if all {
        return vec![]; // empty = no filter = show everything
    }
    if !cli_paths.is_empty() {
        // Use explicit --path overrides, resolve to absolute
        return cli_paths
            .iter()
            .map(|p| {
                let pb = PathBuf::from(p);
                let abs = if pb.is_absolute() {
                    pb
                } else {
                    std::env::current_dir()
                        .unwrap_or_else(|_| PathBuf::from("."))
                        .join(pb)
                };
                abs.canonicalize()
                    .unwrap_or(abs)
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
    }
    // Default: use config's scan_paths resolved + canonicalized
    config
        .resolve_scan_paths(config_dir)
        .iter()
        .map(|p| {
            p.canonicalize()
                .unwrap_or_else(|_| p.clone())
                .to_string_lossy()
                .to_string()
        })
        .collect()
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let config_path = if cli.config.is_absolute() {
        cli.config.clone()
    } else {
        std::env::current_dir()?.join(&cli.config)
    };

    let config_dir = config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    match cli.command {
        Commands::Scan {
            min_size,
            path,
            threads,
            resume,
        } => {
            let mut config = load_config(&config_path)?;
            if let Some(ms) = min_size {
                config.min_size = ms;
            }
            if !path.is_empty() {
                config.scan_paths = path;
            }
            if let Some(t) = threads {
                config.threads = t;
            }

            let db = Db::open(config_dir)?;
            let result = scanner::scan(&config, config_dir, &db, resume)?;

            // Show top files scoped to what was just scanned
            let scope: Vec<String> = config
                .resolve_scan_paths(config_dir)
                .iter()
                .map(|p| {
                    p.canonicalize()
                        .unwrap_or_else(|_| p.clone())
                        .to_string_lossy()
                        .to_string()
                })
                .collect();
            let files = db.get_top_files(config.top_n, Some("exists"), &scope)?;
            display::print_files(&files, "Biggest Files");

            if result.interrupted {
                println!(
                    "  {} Partial results — run {} to finish.\n",
                    "⏸".yellow(),
                    "treeshark scan --resume".bold()
                );
            }
        }

        Commands::List {
            top,
            status,
            all,
            filter_path,
        } => {
            let config = load_config(&config_path)?;
            let db = Db::open(config_dir)?;
            let prefixes = resolve_path_filter(all, &filter_path, &config, config_dir);

            let filter = match status.as_str() {
                "all" => None,
                s => Some(s),
            };
            let files = db.get_top_files(top, filter, &prefixes)?;

            let scope_label = if all {
                " (all paths)".to_string()
            } else if !filter_path.is_empty() {
                format!(" ({})", filter_path.join(", "))
            } else {
                let paths = &prefixes;
                format!(" ({})", paths.join(", "))
            };

            let title = match status.as_str() {
                "all" => format!("All Files{}", scope_label),
                "exists" => format!("Existing Files{}", scope_label),
                "deleted" => format!("Deleted Files{}", scope_label),
                "missing" => format!("Missing Files{}", scope_label),
                s => format!("Files (status={}){}", s, scope_label),
            };
            display::print_files(&files, &title);
        }

        Commands::Delete { all, filter_path } => {
            let config = load_config(&config_path)?;
            let db = Db::open(config_dir)?;
            let prefixes = resolve_path_filter(all, &filter_path, &config, config_dir);
            deleter::interactive_delete(&db, config.top_n, &prefixes)?;
        }

        Commands::Stats => {
            let db = Db::open(config_dir)?;
            let stats = db.stats()?;
            display::print_stats(&stats, &db)?;
        }

        Commands::History { limit } => {
            let db = Db::open(config_dir)?;
            let scans = db.get_scan_history(limit)?;
            display::print_history(&scans);
        }

        Commands::Reset => {
            let db = Db::open(config_dir)?;

            println!();
            let confirm = dialoguer::Confirm::new()
                .with_prompt(format!(
                    "  {} Delete ALL scan data from {}?",
                    "⚠".red().bold(),
                    db.path.display()
                ))
                .default(false)
                .interact()?;

            if confirm {
                db.reset()?;
                println!("  🧹 Database reset.\n");
            } else {
                println!("  {} Aborted.\n", "→".dimmed());
            }
        }

        Commands::Config => {
            let config = load_config(&config_path)?;
            let db_path = Db::db_path(config_dir);
            let resolved: Vec<String> = config
                .resolve_scan_paths(config_dir)
                .iter()
                .map(|p| {
                    p.canonicalize()
                        .unwrap_or_else(|_| p.clone())
                        .to_string_lossy()
                        .to_string()
                })
                .collect();
            println!(
                "\n{}  Config: {}\n",
                "🦈 TREESHARK".bold().cyan(),
                config_path.display().to_string().dimmed()
            );
            println!("  {}     {}", "min_size:".bold(), config.min_size.yellow());
            println!("  {}   {:?}", "scan_paths:".bold(), config.scan_paths);
            println!("  {}   {:?}", "  resolved:".dimmed(), resolved);
            println!("  {}        {}", "top_n:".bold(), config.top_n);
            println!("  {}      {:?}", "exclude:".bold(), config.exclude);
            println!(
                "  {}      {} (0 = all {} cores)",
                "threads:".bold(),
                config.threads.to_string().cyan(),
                num_cpus::get()
            );
            println!(
                "  {}     {}",
                "database:".bold(),
                db_path.display().to_string().dimmed()
            );
            println!();
        }
    }

    Ok(())
}

fn load_config(path: &PathBuf) -> Result<Config> {
    if !path.exists() {
        eprintln!(
            "\n  {} Config not found: {}\n",
            "⚠".yellow().bold(),
            path.display()
        );
        eprintln!("  Creating default config...\n");

        let default_config = Config::default();
        let yaml = serde_yaml::to_string(&default_config)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, yaml)?;

        eprintln!(
            "  {} Default config written to {}\n  Edit it and run again.\n",
            "✓".green(),
            path.display()
        );

        return Ok(default_config);
    }
    Config::load(path)
}
