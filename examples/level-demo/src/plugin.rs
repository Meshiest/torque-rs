//! The level demo as an engine plugin: its files, its maps, and the client's half of the prediction.

use std::rc::Rc;

use torque::app::camera_modes::{
    CameraBuilderExt as _, FIRST_PERSON, FirstPerson, THIRD_PERSON, ThirdPerson,
};
use torque::app::plugins::{ClientGamePlugin, DefaultPlugins, GuiPlugin, ServerQueryPlugin};
use torque::engine::{ClientStage, EngineBuilder, Plugin, PluginGroup as _, PluginId};
use torque_example_common::prediction::CubeClient;

use crate::files::{GameFiles, MapEntry};

/// What [`LevelPlugin`] keeps for the game to read back from the engine: the files (with the levels and the valley built
/// in), the maps the menu offers, and the prediction's answers.
pub struct Level {
    pub files: Rc<GameFiles>,
    pub maps: Vec<MapEntry>,
    pub cube: Rc<CubeClient>,
}

/// The game's part of the engine: the menu's maps as globals of every client, and the client game that predicts the figure
/// ([`CubeClient`]).
#[derive(Clone, PluginId)]
pub struct LevelPlugin {
    level: Rc<Level>,
}

impl LevelPlugin {
    /// The plugin over `files`, the levels and the valley added to them.
    pub fn new(mut files: GameFiles) -> Result<LevelPlugin, String> {
        crate::levels::add_levels(&mut files)?;
        crate::files::add_valley(&mut files)?;
        #[cfg(feature = "rope")]
        crate::ropes::add_textures(&mut files);
        let maps = crate::files::maps(&files);
        let cube = Rc::new(CubeClient::new(files.mem_files()));
        cube.set_poses(crate::figure_poses());
        Ok(LevelPlugin {
            level: Rc::new(Level {
                files: Rc::new(files),
                maps,
                cube,
            }),
        })
    }
}

impl Plugin for LevelPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_plugins(ClientGamePlugin::new(Rc::clone(&self.level.cube) as _));
        engine.client(ClientStage::Natives, |vm, engine| {
            let level = engine.resource::<Rc<Level>>().ok_or("the level demo")?;
            vm.set_global("$World::MapCount", &level.maps.len().to_string());
            for (i, m) in level.maps.iter().enumerate() {
                vm.set_global(&format!("$World::MapTitle{i}"), &m.title);
                vm.set_global(&format!("$World::MapAbout{i}"), &m.about);
            }
            Ok(())
        });
        // The numbers of the camera behind the figure are the example's.
        let rig = crate::client::camera_rig();
        engine.add_camera_mode(FIRST_PERSON, move || FirstPerson::new(rig));
        engine.add_camera_mode(THIRD_PERSON, move || ThirdPerson::new(rig));
        engine.insert_resource(Rc::clone(&self.level));
    }
}

/// The engine of the level demo over `files`: the engine's plugins but the server query, the GUI measuring text with the
/// fonts it is drawn with, and [`LevelPlugin`]. Name the game on it ([`EngineRun::app`](torque::app::run::EngineRun::app)) to
/// run it, or build it for [`LevelDemo::from_engine`](crate::client::LevelDemo::from_engine).
pub fn level_engine(files: GameFiles) -> Result<EngineBuilder, String> {
    let mut engine = EngineBuilder::new();
    engine.add_plugins(
        DefaultPlugins
            .set(GuiPlugin { layout_fonts: true })
            .disable::<ServerQueryPlugin>(),
    );
    engine.add_plugins(LevelPlugin::new(files)?);
    #[cfg(feature = "rope")]
    engine.add_plugins(
        torque_plugin_rope::RopePlugin::default().with_frame_anchors(crate::ropes::Mounts),
    );
    Ok(engine)
}
