use std::ffi::OsString;
use std::path::PathBuf;

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use sumaru::afni::{DEFAULT_AFNI_HOST, resolve_afni_port_config};
use sumaru::inspect::inspect_path;
use sumaru::niml_debug::{
    NimlSendCommand, inspect_debug_path, replay_debug_path, send_debug_command,
};
use sumaru::viewer::{self, AfniViewerOptions, ExplicitOverlayPair};

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

    /// Load a NIfTI volume (.nii/.nii.gz) for orthogonal slice-plane rendering.
    #[arg(long = "volume", visible_alias = "vol", value_name = "PATH")]
    volume: Option<PathBuf>,

    /// Load this GIFTI data array as a per-vertex surface overlay.
    #[arg(long = "overlay", value_name = "PATH")]
    overlay: Option<PathBuf>,

    /// Explicit left-hemisphere overlay for both-hemisphere spec launches.
    #[arg(
        long = "overlay-lh",
        value_name = "PATH",
        conflicts_with = "overlay",
        requires = "overlay_rh"
    )]
    overlay_lh: Option<PathBuf>,

    /// Explicit right-hemisphere overlay for both-hemisphere spec launches.
    #[arg(
        long = "overlay-rh",
        value_name = "PATH",
        conflicts_with = "overlay",
        requires = "overlay_lh"
    )]
    overlay_rh: Option<PathBuf>,

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

    /// Preload all spec surfaces into memory before the viewer opens, so
    /// switching between surfaces is instant (slower startup).
    #[arg(long = "preload")]
    preload: bool,

    /// Deprecated: on-demand spec loading is now the default.
    #[arg(long = "no-preload", hide = true, conflicts_with = "preload")]
    no_preload: bool,

    /// Connect to AFNI/SUMA NIML talk on launch. Press `t` in the viewer to
    /// toggle the same connection interactively.
    #[arg(long = "talk-afni")]
    talk_afni: bool,

    /// Record every live AFNI/SUMA NIML message sent and received by Sumaru.
    #[arg(long = "niml-record", value_name = "PATH")]
    niml_record: Option<PathBuf>,

    /// AFNI/SUMA NIML host.
    #[arg(long = "afni-host", default_value = DEFAULT_AFNI_HOST)]
    afni_host: String,

    /// Explicit AFNI/SUMA NIML port. Overrides --np/--npb and AFNI env vars.
    #[arg(long = "afni-port", value_name = "PORT", conflicts_with_all = ["np", "npb"])]
    afni_port: Option<u16>,

    /// AFNI-style NIML port offset.
    #[arg(long = "np", value_name = "PORT_OFFSET", conflicts_with = "npb")]
    np: Option<u16>,

    /// AFNI-style NIML port bloc.
    #[arg(long = "npb", value_name = "PORT_BLOC", conflicts_with = "np")]
    npb: Option<u16>,

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
    /// Inspect, replay, or send AFNI/SUMA NIML debug messages.
    Niml {
        #[command(subcommand)]
        command: NimlCommands,
    },
}

#[derive(Debug, Subcommand)]
enum NimlCommands {
    /// Print an offline summary of a raw NIML file or Sumaru NIML recording.
    Inspect {
        /// Path to a raw NIML file or a --niml-record trace.
        path: PathBuf,
    },
    /// Replay a raw NIML file or Sumaru NIML recording through the parser/router.
    Replay {
        /// Path to a raw NIML file or a --niml-record trace.
        path: PathBuf,
    },
    /// Send a small NIML test message to an AFNI/SUMA NIML socket.
    Send {
        #[command(subcommand)]
        command: NimlSendCommands,
    },
}

#[derive(Debug, Subcommand)]
enum NimlSendCommands {
    /// Send every NIML element parsed from a file.
    Raw {
        /// Path to a raw NIML file.
        path: PathBuf,
    },
    /// Send a SUMA_crosshair_xyz test message.
    Crosshair {
        /// AFNI/SUMA surface idcode to target.
        #[arg(long = "surface-id")]
        surface_idcode: String,

        /// Optional domain parent idcode.
        #[arg(long = "domain-parent-id")]
        domain_parent_idcode: Option<String>,

        /// Surface-local node index.
        #[arg(long = "node")]
        node_index: u32,

        /// AFNI-space XYZ coordinate, formatted as x,y,z.
        #[arg(long = "xyz", value_parser = parse_xyz)]
        xyz: [f32; 3],
    },
    /// Send a small Sumaru-prefixed viewer command.
    Command {
        /// Command name, for example reset-camera or toggle-overlay.
        #[arg(value_parser = parse_niml_viewer_command)]
        command: String,
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
    let niml_record_path = cli.niml_record;
    let overlay_pair = explicit_overlay_pair(cli.overlay_lh, cli.overlay_rh);
    let afni_requested = cli.talk_afni
        || cli.afni_port.is_some()
        || cli.np.is_some()
        || cli.npb.is_some()
        || cli.afni_host != DEFAULT_AFNI_HOST;
    let environ = std::env::vars().collect::<BTreeMap<_, _>>();
    let afni = AfniViewerOptions {
        connect_on_launch: cli.talk_afni,
        port_config: resolve_afni_port_config(
            cli.afni_host,
            cli.afni_port,
            cli.np,
            cli.npb,
            &environ,
        )?,
    };

    let surface = cli.surface;
    let spec = cli.spec;
    let surface_volume = cli.surface_volume;
    let volume = cli.volume;
    let overlay = cli.overlay;
    let roi = cli.roi;

    match cli.command {
        None => {
            validate_viewer_launch(
                &surface,
                &spec,
                &surface_volume,
                &overlay,
                &overlay_pair,
                &roi,
                &subs,
                &p_value,
            )?;
            viewer::run(viewer::LaunchOptions {
                surface_path: surface,
                spec_path: spec,
                surface_volume_path: surface_volume,
                volume_path: volume,
                overlay_path: overlay,
                overlay_pair_paths: overlay_pair,
                roi_path: roi,
                overlay_subs: subs,
                overlay_p_value: p_value,
                verbose,
                preload,
                afni,
                niml_record_path,
            })?;
        }
        Some(Commands::Inspect { path }) => {
            validate_no_viewer_launch_options(
                &surface,
                &spec,
                &surface_volume,
                &overlay,
                &overlay_pair,
                &roi,
                &subs,
                &p_value,
                &niml_record_path,
            )?;
            if afni_requested {
                bail!("AFNI connection flags only apply to viewer launches and `niml send`");
            }
            let report = inspect_path(path)?;
            println!("{report}");
        }
        Some(Commands::Niml { command }) => {
            validate_no_viewer_launch_options(
                &surface,
                &spec,
                &surface_volume,
                &overlay,
                &overlay_pair,
                &roi,
                &subs,
                &p_value,
                &niml_record_path,
            )?;
            run_niml_command(command, &afni.port_config, verbose)?;
        }
    }

    Ok(())
}

fn run_niml_command(
    command: NimlCommands,
    afni: &sumaru::afni::AfniPortConfig,
    verbose: bool,
) -> Result<()> {
    match command {
        NimlCommands::Inspect { path } => {
            println!("{}", inspect_debug_path(path)?);
        }
        NimlCommands::Replay { path } => {
            println!("{}", replay_debug_path(path)?.to_text());
        }
        NimlCommands::Send { command } => {
            let command = match command {
                NimlSendCommands::Raw { path } => NimlSendCommand::Raw(path),
                NimlSendCommands::Crosshair {
                    surface_idcode,
                    domain_parent_idcode,
                    node_index,
                    xyz,
                } => NimlSendCommand::Crosshair {
                    surface_idcode,
                    domain_parent_idcode,
                    node_index,
                    xyz,
                },
                NimlSendCommands::Command { command } => NimlSendCommand::ViewerCommand(command),
            };
            let count = send_debug_command(afni, verbose, command)?;
            println!(
                "Sent {count} NIML element{} to {}:{}.",
                if count == 1 { "" } else { "s" },
                afni.host,
                afni.port
            );
        }
    }

    Ok(())
}

fn validate_viewer_launch(
    surface: &Option<PathBuf>,
    spec: &Option<PathBuf>,
    surface_volume: &Option<PathBuf>,
    overlay: &Option<PathBuf>,
    overlay_pair: &Option<ExplicitOverlayPair>,
    roi: &Option<PathBuf>,
    subs: &Option<Vec<String>>,
    p_value: &Option<f64>,
) -> Result<()> {
    let has_overlay = overlay.is_some() || overlay_pair.is_some();
    if surface.is_some() && spec.is_some() {
        bail!("use either -i/--surface or -spec/--spec, not both");
    }
    if spec.is_some() && surface_volume.is_none() {
        bail!("-spec/--spec requires -sv/--sv");
    }
    if surface.is_none() && spec.is_none() && overlay.is_some() {
        bail!("--overlay requires -i/--surface or -spec/--spec");
    }
    if overlay_pair.is_some() && spec.is_none() {
        bail!("--overlay-lh/--overlay-rh require -spec/--spec");
    }
    if surface.is_none() && spec.is_none() && roi.is_some() {
        bail!("--roi requires -i/--surface or -spec/--spec");
    }
    if surface.is_none() && spec.is_none() && surface_volume.is_some() {
        bail!("-sv/--sv requires -i/--surface or -spec/--spec");
    }
    if !has_overlay && subs.is_some() {
        bail!("--subs requires --overlay or --overlay-lh/--overlay-rh");
    }
    if !has_overlay && p_value.is_some() {
        bail!("--p-val requires --overlay or --overlay-lh/--overlay-rh");
    }

    Ok(())
}

fn validate_no_viewer_launch_options(
    surface: &Option<PathBuf>,
    spec: &Option<PathBuf>,
    surface_volume: &Option<PathBuf>,
    overlay: &Option<PathBuf>,
    overlay_pair: &Option<ExplicitOverlayPair>,
    roi: &Option<PathBuf>,
    subs: &Option<Vec<String>>,
    p_value: &Option<f64>,
    niml_record_path: &Option<PathBuf>,
) -> Result<()> {
    if surface.is_some()
        || spec.is_some()
        || surface_volume.is_some()
        || overlay.is_some()
        || overlay_pair.is_some()
        || roi.is_some()
        || subs.is_some()
        || p_value.is_some()
        || niml_record_path.is_some()
    {
        bail!("viewer launch options and subcommands cannot be mixed");
    }

    Ok(())
}

fn explicit_overlay_pair(
    left_path: Option<PathBuf>,
    right_path: Option<PathBuf>,
) -> Option<ExplicitOverlayPair> {
    match (left_path, right_path) {
        (Some(left_path), Some(right_path)) => Some(ExplicitOverlayPair {
            left_path,
            right_path,
        }),
        _ => None,
    }
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

fn parse_xyz(value: &str) -> Result<[f32; 3], String> {
    let pieces = value
        .split(',')
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
        .map(|piece| {
            piece
                .parse::<f32>()
                .map_err(|_| format!("'{piece}' is not a valid XYZ coordinate"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if pieces.len() != 3 {
        return Err("--xyz expects x,y,z".to_string());
    }
    if pieces.iter().any(|value| !value.is_finite()) {
        return Err("--xyz coordinates must be finite".to_string());
    }
    Ok([pieces[0], pieces[1], pieces[2]])
}

fn parse_niml_viewer_command(value: &str) -> Result<String, String> {
    let normalized = value.trim().replace('-', "_");
    match normalized.as_str() {
        "reset_camera"
        | "toggle_overlay"
        | "background_black"
        | "background_white"
        | "surface_controller_open"
        | "surface_controller_closed"
        | "roi_controller_open"
        | "roi_controller_closed" => Ok(normalized),
        _ => Err(format!(
            "unknown NIML viewer command '{value}'; expected one of reset-camera, \
             toggle-overlay, background-black, background-white, surface-controller-open, \
             surface-controller-closed, roi-controller-open, roi-controller-closed"
        )),
    }
}

fn normalized_afni_style_args() -> Vec<OsString> {
    std::env::args_os().map(normalize_afni_style_arg).collect()
}

fn normalize_afni_style_arg(arg: OsString) -> OsString {
    if arg == "-spec" {
        OsString::from("--spec")
    } else if arg == "-sv" {
        OsString::from("--sv")
    } else if arg == "-np" {
        OsString::from("--np")
    } else if arg == "-npb" {
        OsString::from("--npb")
    } else {
        arg
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Cli, Commands, NimlCommands, NimlSendCommands, SubSpec, explicit_overlay_pair,
        normalize_afni_style_arg, parse_niml_viewer_command, parse_p_value, parse_subs, parse_xyz,
        validate_no_viewer_launch_options, validate_viewer_launch,
    };
    use clap::Parser;
    use std::ffi::OsString;
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
    fn explicit_paired_overlay_launch_options_parse_and_validate() {
        let cli = Cli::parse_from([
            "sumaru",
            "--spec",
            "scene_both.spec",
            "--sv",
            "SurfVol.nii",
            "--overlay-lh",
            "left.weird.name.niml.dset",
            "--overlay-rh",
            "right.other.name.niml.dset",
            "--subs",
            "0,1",
            "--p-val",
            "0.01",
        ]);

        let overlay_pair = explicit_overlay_pair(cli.overlay_lh, cli.overlay_rh);
        assert_eq!(
            overlay_pair.as_ref().map(|pair| &pair.left_path),
            Some(&PathBuf::from("left.weird.name.niml.dset"))
        );
        assert_eq!(
            overlay_pair.as_ref().map(|pair| &pair.right_path),
            Some(&PathBuf::from("right.other.name.niml.dset"))
        );
        assert!(
            validate_viewer_launch(
                &cli.surface,
                &cli.spec,
                &cli.surface_volume,
                &cli.overlay,
                &overlay_pair,
                &cli.roi,
                &cli.subs.map(|subs| subs.0),
                &cli.p_value,
            )
            .is_ok()
        );

        assert!(Cli::try_parse_from(["sumaru", "--overlay-lh", "left.niml.dset"]).is_err());
        assert!(
            Cli::try_parse_from([
                "sumaru",
                "--surface",
                "surface.gii",
                "--overlay",
                "stats.niml.dset",
                "--overlay-lh",
                "left.niml.dset",
                "--overlay-rh",
                "right.niml.dset",
            ])
            .is_err()
        );

        let pair = explicit_overlay_pair(path("left.niml.dset"), path("right.niml.dset"));
        assert!(
            validate_viewer_launch(
                &path("surface.gii"),
                &None,
                &None,
                &None,
                &pair,
                &None,
                &None,
                &None,
            )
            .is_err()
        );
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

    #[test]
    fn afni_talk_launch_options_parse() {
        let cli = Cli::parse_from([
            "sumaru",
            "--surface",
            "surface.gii",
            "--talk-afni",
            "--afni-port",
            "53211",
        ]);

        assert!(cli.talk_afni);
        assert_eq!(cli.afni_port, Some(53211));
    }

    #[test]
    fn niml_record_path_is_viewer_only() {
        let cli = Cli::parse_from([
            "sumaru",
            "--surface",
            "surface.gii",
            "--niml-record",
            "session.nimlrec",
        ]);
        assert_eq!(cli.niml_record, Some(PathBuf::from("session.nimlrec")));

        assert!(
            validate_no_viewer_launch_options(
                &None,
                &None,
                &None,
                &None,
                &None,
                &None,
                &None,
                &None,
                &Some(PathBuf::from("session.nimlrec")),
            )
            .is_err()
        );
    }

    #[test]
    fn niml_subcommands_parse() {
        let cli = Cli::parse_from(["sumaru", "niml", "inspect", "session.nimlrec"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Niml {
                command: NimlCommands::Inspect { .. }
            })
        ));

        let cli = Cli::parse_from([
            "sumaru",
            "--afni-port",
            "53211",
            "niml",
            "send",
            "crosshair",
            "--surface-id",
            "surf",
            "--node",
            "42",
            "--xyz",
            "1,2,3",
        ]);
        assert_eq!(cli.afni_port, Some(53211));
        assert!(matches!(
            cli.command,
            Some(Commands::Niml {
                command: NimlCommands::Send {
                    command: NimlSendCommands::Crosshair {
                        surface_idcode,
                        node_index: 42,
                        xyz,
                        ..
                    }
                }
            }) if surface_idcode == "surf" && xyz == [1.0, 2.0, 3.0]
        ));
    }

    #[test]
    fn niml_send_parsers_validate_commands_and_xyz() {
        assert_eq!(parse_xyz("1,2.5,-3").unwrap(), [1.0, 2.5, -3.0]);
        assert!(parse_xyz("1,2").is_err());
        assert!(parse_xyz("1,nan,3").is_err());

        assert_eq!(
            parse_niml_viewer_command("reset-camera").unwrap(),
            "reset_camera"
        );
        assert_eq!(
            parse_niml_viewer_command("toggle_overlay").unwrap(),
            "toggle_overlay"
        );
        assert!(parse_niml_viewer_command("do-anything").is_err());
    }

    #[test]
    fn afni_style_port_flags_normalize_for_clap() {
        assert_eq!(
            normalize_afni_style_arg(OsString::from("-np")),
            OsString::from("--np")
        );
        assert_eq!(
            normalize_afni_style_arg(OsString::from("-npb")),
            OsString::from("--npb")
        );

        let cli = Cli::parse_from([
            OsString::from("sumaru"),
            normalize_afni_style_arg(OsString::from("-spec")),
            OsString::from("scene.spec"),
            normalize_afni_style_arg(OsString::from("-sv")),
            OsString::from("SurfVol.nii"),
            normalize_afni_style_arg(OsString::from("-npb")),
            OsString::from("1"),
        ]);
        assert_eq!(cli.npb, Some(1));
    }
}
