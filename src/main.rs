use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use sumaru::inspect::inspect_path;
use sumaru::viewer;

#[derive(Debug, Parser)]
#[command(version, about = "SUMA in Rust")]
struct Cli {
    /// Launch the viewer with this GIFTI surface.
    #[arg(short = 'i', long = "surface", value_name = "PATH")]
    surface: Option<PathBuf>,

    /// Load this GIFTI data array as a per-vertex surface overlay.
    #[arg(long = "overlay", value_name = "PATH")]
    overlay: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Read a supported neuroimaging file and print a short summary.
    Inspect {
        /// Path to a GIFTI or NIFTI file.
        path: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match (cli.surface, cli.overlay, cli.command) {
        (Some(path), overlay, None) => viewer::run(Some(path), overlay)?,
        (None, None, Some(Commands::Inspect { path })) => {
            let report = inspect_path(path)?;
            println!("{report}");
        }
        (None, None, None) => viewer::run(None, None)?,
        (None, Some(_), _) => {
            bail!("--overlay requires -i/--surface");
        }
        (Some(_), _, Some(_)) => {
            bail!("use -i/--surface with optional --overlay, or use a subcommand, not both");
        }
    }

    Ok(())
}
