use std::ffi::OsString;
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

    /// Launch the viewer from this SUMA spec file.
    #[arg(long = "spec", value_name = "PATH")]
    spec: Option<PathBuf>,

    /// Surface-volume context for AFNI/NIML communication.
    #[arg(long = "sv", value_name = "PATH")]
    surface_volume: Option<PathBuf>,

    /// Load this GIFTI data array as a per-vertex surface overlay.
    #[arg(long = "overlay", value_name = "PATH")]
    overlay: Option<PathBuf>,

    /// Print viewer status messages to the terminal.
    #[arg(long = "verbose")]
    verbose: bool,

    /// Preload spec surfaces in the background after the first display state.
    #[arg(long = "preload")]
    preload: bool,

    /// Deprecated: on-demand spec loading is now the default.
    #[arg(long = "no-preload", hide = true, conflicts_with = "preload")]
    no_preload: bool,

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
    let cli = Cli::parse_from(normalized_afni_style_args());
    let verbose = cli.verbose;
    let preload = cli.preload && !cli.no_preload;

    match (
        cli.surface,
        cli.spec,
        cli.surface_volume,
        cli.overlay,
        cli.command,
    ) {
        (surface, spec, surface_volume, overlay, None) => {
            validate_viewer_launch(&surface, &spec, &surface_volume, &overlay)?;
            viewer::run(viewer::LaunchOptions {
                surface_path: surface,
                spec_path: spec,
                surface_volume_path: surface_volume,
                overlay_path: overlay,
                verbose,
                preload,
            })?;
        }
        (None, None, None, None, Some(Commands::Inspect { path })) => {
            let report = inspect_path(path)?;
            println!("{report}");
        }
        _ => {
            bail!("viewer launch options and subcommands cannot be mixed");
        }
    }

    Ok(())
}

fn validate_viewer_launch(
    surface: &Option<PathBuf>,
    spec: &Option<PathBuf>,
    surface_volume: &Option<PathBuf>,
    overlay: &Option<PathBuf>,
) -> Result<()> {
    if surface.is_some() && spec.is_some() {
        bail!("use either -i/--surface or -spec/--spec, not both");
    }
    if spec.is_some() && surface_volume.is_none() {
        bail!("-spec/--spec requires -sv/--sv");
    }
    if surface.is_none() && spec.is_none() && overlay.is_some() {
        bail!("--overlay requires -i/--surface or -spec/--spec");
    }
    if surface.is_none() && spec.is_none() && surface_volume.is_some() {
        bail!("-sv/--sv requires -i/--surface or -spec/--spec");
    }

    Ok(())
}

fn normalized_afni_style_args() -> Vec<OsString> {
    std::env::args_os()
        .map(|arg| {
            if arg == "-spec" {
                OsString::from("--spec")
            } else if arg == "-sv" {
                OsString::from("--sv")
            } else {
                arg
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Cli, validate_viewer_launch};
    use clap::Parser;
    use std::path::PathBuf;

    fn path(value: &str) -> Option<PathBuf> {
        Some(PathBuf::from(value))
    }

    #[test]
    fn regular_surface_launch_does_not_require_spec_or_surface_volume() {
        assert!(validate_viewer_launch(&path("surface.gii"), &None, &None, &None).is_ok());
    }

    #[test]
    fn spec_launch_requires_surface_volume_context() {
        assert!(validate_viewer_launch(&None, &path("surface.spec"), &None, &None).is_err());
        assert!(
            validate_viewer_launch(&None, &path("surface.spec"), &path("SurfVol.nii"), &None)
                .is_ok()
        );
    }

    #[test]
    fn surface_volume_can_attach_to_direct_surface_launch() {
        assert!(
            validate_viewer_launch(&path("surface.gii"), &None, &path("SurfVol.nii"), &None)
                .is_ok()
        );
    }

    #[test]
    fn surface_volume_without_surface_context_is_rejected() {
        assert!(validate_viewer_launch(&None, &None, &path("SurfVol.nii"), &None).is_err());
    }

    #[test]
    fn overlay_still_requires_a_surface_context() {
        assert!(validate_viewer_launch(&None, &None, &None, &path("stats.niml.dset")).is_err());
    }

    #[test]
    fn surface_and_spec_remain_mutually_exclusive() {
        assert!(
            validate_viewer_launch(&path("surface.gii"), &path("surface.spec"), &None, &None)
                .is_err()
        );
    }

    #[test]
    fn spec_preload_is_opt_in() {
        let cli = Cli::parse_from(["sumaru", "--spec", "scene.spec", "--sv", "SurfVol.nii"]);
        assert!(!cli.preload);
        assert!(!cli.no_preload);

        let cli = Cli::parse_from([
            "sumaru",
            "--spec",
            "scene.spec",
            "--sv",
            "SurfVol.nii",
            "--preload",
        ]);
        assert!(cli.preload);
        assert!(!cli.no_preload);
    }

    #[test]
    fn deprecated_no_preload_flag_is_a_compatible_no_op() {
        let cli = Cli::parse_from([
            "sumaru",
            "--spec",
            "scene.spec",
            "--sv",
            "SurfVol.nii",
            "--no-preload",
        ]);

        assert!(!cli.preload);
        assert!(cli.no_preload);
    }
}
