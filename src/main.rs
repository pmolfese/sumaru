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

    /// Load this SUMA .niml.roi annotation over the active surface.
    #[arg(long = "roi", value_name = "PATH")]
    roi: Option<PathBuf>,

    /// Initial overlay sub-bricks as I,T[,B], using zero-based column numbers
    /// or column labels.
    #[arg(long = "subs", value_name = "I,T[,B]", value_parser = parse_subs)]
    subs: Option<SubSpec>,

    /// Initial overlay p-value threshold, converted through the selected T
    /// sub-brick's AFNI stat metadata.
    #[arg(long = "p-val", value_name = "P", value_parser = parse_p_value)]
    p_value: Option<f64>,

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct SubSpec(Vec<String>);

fn main() -> Result<()> {
    let cli = Cli::parse_from(normalized_afni_style_args());
    let verbose = cli.verbose;
    let preload = cli.preload && !cli.no_preload;
    let subs = cli.subs.map(|subs| subs.0);
    let p_value = cli.p_value;

    match (
        cli.surface,
        cli.spec,
        cli.surface_volume,
        cli.overlay,
        cli.roi,
        subs,
        p_value,
        cli.command,
    ) {
        (surface, spec, surface_volume, overlay, roi, subs, p_value, None) => {
            validate_viewer_launch(
                &surface,
                &spec,
                &surface_volume,
                &overlay,
                &roi,
                &subs,
                &p_value,
            )?;
            viewer::run(viewer::LaunchOptions {
                surface_path: surface,
                spec_path: spec,
                surface_volume_path: surface_volume,
                overlay_path: overlay,
                roi_path: roi,
                overlay_subs: subs,
                overlay_p_value: p_value,
                verbose,
                preload,
            })?;
        }
        (None, None, None, None, None, None, None, Some(Commands::Inspect { path })) => {
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
    roi: &Option<PathBuf>,
    subs: &Option<Vec<String>>,
    p_value: &Option<f64>,
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
    if surface.is_none() && spec.is_none() && roi.is_some() {
        bail!("--roi requires -i/--surface or -spec/--spec");
    }
    if surface.is_none() && spec.is_none() && surface_volume.is_some() {
        bail!("-sv/--sv requires -i/--surface or -spec/--spec");
    }
    if overlay.is_none() && subs.is_some() {
        bail!("--subs requires --overlay");
    }
    if overlay.is_none() && p_value.is_some() {
        bail!("--p-val requires --overlay");
    }

    Ok(())
}

fn parse_subs(value: &str) -> Result<SubSpec, String> {
    let pieces = value
        .split(',')
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if !(2..=3).contains(&pieces.len()) {
        return Err("--subs expects I,T or I,T,B".to_string());
    }

    Ok(SubSpec(pieces))
}

fn parse_p_value(value: &str) -> Result<f64, String> {
    let p_value = value
        .parse::<f64>()
        .map_err(|_| format!("'{value}' is not a valid p-value"))?;
    if !(0.0..=1.0).contains(&p_value) || !p_value.is_finite() || p_value == 0.0 {
        return Err("--p-val must be greater than 0 and less than or equal to 1".to_string());
    }

    Ok(p_value)
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
    use super::{Cli, SubSpec, parse_p_value, parse_subs, validate_viewer_launch};
    use clap::Parser;
    use std::path::PathBuf;

    fn path(value: &str) -> Option<PathBuf> {
        Some(PathBuf::from(value))
    }

    #[test]
    fn regular_surface_launch_does_not_require_spec_or_surface_volume() {
        assert!(
            validate_viewer_launch(
                &path("surface.gii"),
                &None,
                &None,
                &None,
                &None,
                &None,
                &None
            )
            .is_ok()
        );
    }

    #[test]
    fn spec_launch_requires_surface_volume_context() {
        assert!(
            validate_viewer_launch(
                &None,
                &path("surface.spec"),
                &None,
                &None,
                &None,
                &None,
                &None
            )
            .is_err()
        );
        assert!(
            validate_viewer_launch(
                &None,
                &path("surface.spec"),
                &path("SurfVol.nii"),
                &None,
                &None,
                &None,
                &None
            )
            .is_ok()
        );
    }

    #[test]
    fn surface_volume_can_attach_to_direct_surface_launch() {
        assert!(
            validate_viewer_launch(
                &path("surface.gii"),
                &None,
                &path("SurfVol.nii"),
                &None,
                &None,
                &None,
                &None
            )
            .is_ok()
        );
    }

    #[test]
    fn surface_volume_without_surface_context_is_rejected() {
        assert!(
            validate_viewer_launch(
                &None,
                &None,
                &path("SurfVol.nii"),
                &None,
                &None,
                &None,
                &None
            )
            .is_err()
        );
    }

    #[test]
    fn overlay_still_requires_a_surface_context() {
        assert!(
            validate_viewer_launch(
                &None,
                &None,
                &None,
                &path("stats.niml.dset"),
                &None,
                &None,
                &None
            )
            .is_err()
        );
    }

    #[test]
    fn roi_requires_surface_context() {
        assert!(
            validate_viewer_launch(
                &None,
                &None,
                &None,
                &None,
                &path("roi.niml.roi"),
                &None,
                &None
            )
            .is_err()
        );
        assert!(
            validate_viewer_launch(
                &path("surface.gii"),
                &None,
                &None,
                &None,
                &path("roi.niml.roi"),
                &None,
                &None
            )
            .is_ok()
        );
    }

    #[test]
    fn surface_and_spec_remain_mutually_exclusive() {
        assert!(
            validate_viewer_launch(
                &path("surface.gii"),
                &path("surface.spec"),
                &None,
                &None,
                &None,
                &None,
                &None
            )
            .is_err()
        );
    }

    #[test]
    fn overlay_launch_options_parse_subs_and_p_value() {
        let cli = Cli::parse_from([
            "sumaru",
            "--surface",
            "surface.gii",
            "--overlay",
            "stats.niml.dset",
            "--subs",
            "0,1,Grp_B",
            "--p-val",
            "0.05",
        ]);

        assert_eq!(
            cli.subs,
            Some(SubSpec(vec![
                "0".to_string(),
                "1".to_string(),
                "Grp_B".to_string()
            ]))
        );
        assert_eq!(cli.p_value, Some(0.05));
    }

    #[test]
    fn subs_and_p_value_require_overlay() {
        assert!(
            validate_viewer_launch(
                &path("surface.gii"),
                &None,
                &None,
                &None,
                &None,
                &Some(vec!["0".to_string(), "1".to_string()]),
                &None
            )
            .is_err()
        );
        assert!(
            validate_viewer_launch(
                &path("surface.gii"),
                &None,
                &None,
                &None,
                &None,
                &None,
                &Some(0.05)
            )
            .is_err()
        );
    }

    #[test]
    fn subs_and_p_value_parsers_validate_shape() {
        assert_eq!(
            parse_subs("0,1").unwrap(),
            SubSpec(vec!["0".to_string(), "1".to_string()])
        );
        assert_eq!(
            parse_subs("Grp HV,Grp HV t").unwrap(),
            SubSpec(vec!["Grp HV".to_string(), "Grp HV t".to_string()])
        );
        assert!(parse_subs("0").is_err());
        assert!(parse_subs("0,1,2,3").is_err());

        assert_eq!(parse_p_value("0.05").unwrap(), 0.05);
        assert!(parse_p_value("0").is_err());
        assert!(parse_p_value("1.5").is_err());
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
