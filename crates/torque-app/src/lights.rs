//! The engine's own light and sky ghosts in a game's view: the spot lights where their ghosts are (one mounted on a shape rides
//! its `mount<N>` node), and the day the client runs from the server's TimeOfDay under the first ScatterSky's look (the sun,
//! the ambient light, the fog's color and the physical sky made for the sun) ([`WorldLights`], over
//! [`torque_render_scene::hd::environment`]). A world without them leaves the view as it was: no spot light, the
//! [`crate::view::Daylight`] the game gave.

use torque_ghost_world::tge::TgeClientSet;
use torque_ghost_world::world::ClientWorld;
use torque_render_scene::Assets;
use torque_render_scene::hd::environment::{ClientDay, scatter_look, spot_lights};
use torque_render_scene::hd::scatter::ScatterLook;
use torque_render_scene::shapes::Models;

use crate::view::WorldView;

/// The spot lights and the day of a client's world, frame after frame.
#[derive(Default)]
pub struct WorldLights {
    pub day: ClientDay,
    /// The look the day was last lit under (the default before a ScatterSky is ghosted).
    pub look: ScatterLook,
    models: Models,
    notes: Vec<String>,
}

impl WorldLights {
    /// Forgets the day (a new connection's TimeOfDay starts it again).
    pub fn clear(&mut self) {
        self.day = ClientDay::default();
    }

    /// The day run on `dt` seconds (from the server's TimeOfDay) and the ScatterSky's look taken (no view needed).
    pub fn update(&mut self, world: &ClientWorld<TgeClientSet>, dt: f32) {
        self.day.update(world, dt);
        if let Some(look) = scatter_look(world) {
            self.look = look;
        }
    }

    /// Where the sun (or the moon) is now, towards it; none without a day.
    pub fn sun(&self) -> Option<[f32; 3]> {
        self.day.day.as_ref().map(|d| d.towards())
    }

    /// The spot lights of `world` that are on, where the frame draws them.
    pub fn spots(
        &mut self,
        assets: &dyn Assets,
        world: &ClientWorld<TgeClientSet>,
    ) -> Vec<torque_render_scene::hd::HdSpot> {
        let spots = spot_lights(world, assets, &mut self.models, &mut self.notes);
        self.notes.clear();
        spots
    }

    /// The view given the spot lights that are on and the day's light under the ScatterSky's look (the view's
    /// [`WorldView::set_day`] lights it again only as the sun moves on).
    pub fn apply(
        &mut self,
        view: &mut WorldView,
        assets: &dyn Assets,
        world: &ClientWorld<TgeClientSet>,
    ) {
        view.set_day(&self.day, &self.look);
        let spots = self.spots(assets, world);
        view.set_spots(spots);
    }
}
