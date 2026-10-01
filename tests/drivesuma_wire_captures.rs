use std::path::PathBuf;

use anyhow::Result;
use sumaru::afni::{
    AfniIncomingMessage, DriveSumaAction, DriveSumaColorMap, DriveSumaCommandMode,
    parse_incoming_message_with_drivesuma_mode,
};
use sumaru::command::ViewerCommand;
use sumaru::io::parse_niml_str;
use sumaru::surface::SurfaceSide;

fn compatibility_messages(capture: &str) -> Result<Vec<AfniIncomingMessage>> {
    parse_niml_str(capture)?
        .iter()
        .filter_map(|element| {
            parse_incoming_message_with_drivesuma_mode(
                element,
                DriveSumaCommandMode::SumaCompatibility,
            )
            .transpose()
        })
        .collect()
}

#[test]
fn real_fatcat_roi_load_capture_maps_dataset_palette_and_dim() -> Result<()> {
    let messages = compatibility_messages(include_str!("fixtures/drivesuma/fatcat_roi_load.niml"))?;

    assert_eq!(
        messages,
        vec![AfniIncomingMessage::DriveSumaCommands(vec![
            DriveSumaAction::SelectSurface("Net_000.gii".to_string()),
            DriveSumaAction::LoadDataset(PathBuf::from(
                "/Users/molfesepj/FATCAT_DEMO/Net_000.cols.niml.dset"
            )),
            DriveSumaAction::SetColorMap(DriveSumaColorMap::RoiI32),
            DriveSumaAction::SetDim(0.3),
        ])]
    );
    Ok(())
}

#[test]
fn real_fatcat_amber_and_key_capture_preserves_command_order() -> Result<()> {
    let messages =
        compatibility_messages(include_str!("fixtures/drivesuma/fatcat_amber_keys.niml"))?;

    assert_eq!(
        messages,
        vec![
            AfniIncomingMessage::DriveSumaCommands(vec![
                DriveSumaAction::SelectSurface("Net_000.gii".to_string()),
                DriveSumaAction::SetColorMap(DriveSumaColorMap::AmberMonochrome),
                DriveSumaAction::SetDim(1.0),
            ]),
            AfniIncomingMessage::ViewerCommands(vec![
                ViewerCommand::CycleSceneSurface(1),
                ViewerCommand::ToggleAfniTalk,
            ]),
        ]
    );
    Ok(())
}

#[test]
fn real_fatcat_window_capture_ignores_window_geometry_but_keeps_other_commands() -> Result<()> {
    let messages =
        compatibility_messages(include_str!("fixtures/drivesuma/fatcat_window_keys.niml"))?;

    assert_eq!(
        messages,
        vec![
            AfniIncomingMessage::DriveSumaCommands(vec![
                DriveSumaAction::SetSurfaceControllerVisible(true),
            ]),
            AfniIncomingMessage::ViewerCommands(vec![
                ViewerCommand::ToggleSumaComponentVisibility(SurfaceSide::Left),
                ViewerCommand::ToggleSumaComponentVisibility(SurfaceSide::Right),
                ViewerCommand::ToggleBackground,
            ]),
        ]
    );
    Ok(())
}

#[test]
fn real_kill_suma_capture_requests_compatibility_shutdown_only() -> Result<()> {
    let capture = include_str!("fixtures/drivesuma/kill_suma.niml");
    assert_eq!(
        compatibility_messages(capture)?,
        vec![AfniIncomingMessage::DriveSumaCommands(vec![
            DriveSumaAction::Quit,
        ])]
    );

    let native_messages = parse_niml_str(capture)?
        .iter()
        .filter_map(|element| {
            parse_incoming_message_with_drivesuma_mode(element, DriveSumaCommandMode::Sumaru)
                .transpose()
        })
        .collect::<Result<Vec<_>>>()?;
    assert!(native_messages.is_empty());
    Ok(())
}
