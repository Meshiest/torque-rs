//! The level demo as an engine plugin: its files, its maps, and the client's half of the prediction.
//!
//! **The engine** ([`level_engine`]) speaks the engine's own protocol (`EngineProtocolPlugin`, whatever `TORQUE_NET_PROTOCOL`
//! says): the levels' furniture are props (`torque-plugin-props`), which only that protocol carries, and the client predicts
//! the props its figure pushes (`PropPredictionPlugin`).

use std::rc::Rc;

use torque::app::camera_modes::{
    CameraBuilderExt as _, FIRST_PERSON, FirstPerson, THIRD_PERSON, ThirdPerson,
};
use torque::app::plugins::{
    ClientGamePlugin, DefaultPlugins, GuiPlugin, ServerQueryPlugin, set_protocol,
};
use torque::engine::command::CommandSpec;
use torque::engine::{ClientStage, EngineBuilder, Plugin, PluginGroup, PluginId, ProtocolKind};
use torque_example_common::prediction::CubeClient;
use torque_plugin_props::PropsPlugin;
use torque_plugin_props::predict::PropPredictionPlugin;

use crate::files::{GameFiles, MapEntry};

/// What [`LevelPlugin`] keeps for the game to read back from the engine: the files (with the levels and the valley built
/// in), the maps the menu offers, and the prediction's answers.
pub struct Level {
    pub files: Rc<GameFiles>,
    pub maps: Vec<MapEntry>,
    pub cube: Rc<CubeClient>,
    /// Files only the server has (a path and its bytes each): the mods it requires of its clients, which they download. None
    /// in [`level_engine`].
    pub server_mods: Vec<(String, Vec<u8>)>,
}

/// The game's part of the engine: the menu's maps as globals of every client, and the client game that predicts the figure
/// ([`CubeClient`]).
#[derive(Clone, PluginId)]
pub struct LevelPlugin {
    level: Rc<Level>,
}

impl LevelPlugin {
    /// The plugin over `files`, the levels and the valley added to them.
    pub fn new(files: GameFiles) -> Result<LevelPlugin, String> {
        LevelPlugin::with_mods(files, Vec::new())
    }

    /// [`LevelPlugin::new`], the server also holding the files `server_mods` (a path and its bytes each).
    pub fn with_mods(
        mut files: GameFiles,
        server_mods: Vec<(String, Vec<u8>)>,
    ) -> Result<LevelPlugin, String> {
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
                server_mods,
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
        // A click picks a prop up or drops it (the game's `serverCmdPropGrab`): a few a second is plenty.
        engine.server_command_limits("PropGrab", CommandSpec::new().rate(4, 8));
    }
}

/// The engine of the level demo over `files`: the engine's plugins but the server query, the GUI measuring text with the
/// fonts it is drawn with, the engine's own protocol, the props and their prediction (module docs), and [`LevelPlugin`].
/// Name the game on it ([`EngineRun::app`](torque::app::run::EngineRun::app)) to run it, or build it for
/// [`LevelDemo::from_engine`](crate::client::LevelDemo::from_engine).
pub fn level_engine(files: GameFiles) -> Result<EngineBuilder, String> {
    engine_of(
        LevelPlugin::new(files)?,
        DefaultPlugins
            .set(GuiPlugin { layout_fonts: true })
            .disable::<ServerQueryPlugin>(),
    )
}

/// [`level_engine`] with `plugins` as its default plugins and `level` as the game.
pub(crate) fn engine_of(
    level: LevelPlugin,
    plugins: impl PluginGroup,
) -> Result<EngineBuilder, String> {
    let mut engine = EngineBuilder::new();
    engine.add_plugins(plugins);
    set_protocol(&mut engine, ProtocolKind::Engine);
    engine.add_plugins((PropsPlugin, PropPredictionPlugin::default()));
    engine.add_plugins(level);
    #[cfg(feature = "rope")]
    engine.add_plugins((
        torque_plugin_rope::RopePlugin::default().with_frame_anchors(crate::ropes::Mounts),
        torque_plugin_props::JointRopePlugin,
    ));
    #[cfg(feature = "smooth-prediction")]
    engine.add_plugins(
        torque_plugin_smooth_prediction::SmoothPredictionPlugin::optional(
            torque_plugin_smooth_prediction::SmoothPrediction::default(),
        ),
    );
    Ok(engine)
}
