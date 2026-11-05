//! The client's ghosts as the world view's objects: the engine's ([`GhostScene`]): the figure where the client predicts it (on
//! foot, playing the actions its shape has for the way it moves; seated, at its seat), and the buggy where the client runs
//! it, its tires at their hubs, turned by the steering, spun by the wheels and let down their springs as the client's
//! `extendWheels` found them. The terrain is the view's ground, not a ghost ([`torque::app::WorldView::set_terrain`]).
//!
//! The buggy's tires kick up their dust as the engine runs TGE's tire emitter, with a floor under the slip ([`DUST_MIN_SLIP`]) so
//! a rolling tire throws a little too, and they leave tracks on the terrain ([`track_rules`], the engine's tire tracks laid on
//! the terrain's triangles).

use torque::app::ghosts::{FrameInput, GhostOptions, GhostScene, OwnView};
use torque::app::terrain::Terrain;
use torque::app::view::WorldView;
use torque::client::conn::tge::TgeWorld;
use torque::client::predict::ghost_players::GhostPlayers;
use torque_example_common::KitAssets;
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::source::Eye;
use torque_render_scene::tracks::TrackRules;
use torque_render_scene::vehicle_dust::DustRules;

/// The least slip a tire on the ground throws dust by (TGE's rule throws none from a tire that grips).
pub const DUST_MIN_SLIP: f32 = 0.4;

/// The buggy's tracks: a strip narrower than the tire (its tread's middle prints deepest), laid every 0.4 units, kept 30
/// seconds and fading over the last 12.
pub fn track_rules() -> TrackRules {
    TrackRules {
        width: 0.34,
        spacing: 0.4,
        texture: "kit/track.png".to_string(),
        lifetime: 30.0,
        fade: 12.0,
        max_segments: 4096,
    }
}

/// The objects of the client's ghosts, frame after frame.
pub struct WorldScene {
    ghosts: GhostScene,
}

impl Default for WorldScene {
    fn default() -> WorldScene {
        let mut ghosts = GhostScene::new(GhostOptions::default());
        ghosts.dust.rules = DustRules {
            min_slip: DUST_MIN_SLIP,
        };
        ghosts.set_tire_tracks(Some(track_rules()));
        WorldScene { ghosts }
    }
}

impl WorldScene {
    /// Forgets every ghost's object (a new connection's ghosts are new).
    pub fn clear(&mut self) {
        self.ghosts.clear();
    }

    /// The client's own figure as the last frame placed it.
    pub fn own(&self) -> Option<OwnView> {
        self.ghosts.own()
    }

    /// The tire tracks laid (none before the first frame).
    pub fn track_segments(&self) -> usize {
        self.ghosts.tracks.as_ref().map_or(0, |t| t.len())
    }

    /// The dust particles alive.
    pub fn dust_particles(&self) -> usize {
        self.ghosts.dust.particles()
    }

    /// The tires' dust particles alive: where each was made (where it is less its velocity over its age) and its age (ms).
    pub fn dust_spots(&self) -> Vec<([f32; 3], i32)> {
        self.ghosts
            .dust
            .alive()
            .filter(|(tire, _)| *tire)
            .map(|(_, p)| {
                let t = p.age_ms as f32 / 1000.0;
                ([0, 1, 2].map(|k| p.pos[k] - p.vel[k] * t), p.age_ms)
            })
            .collect()
    }

    /// The tire tracks' heads (from each strip's last segment to its wheel): their points.
    pub fn track_heads(&self) -> Vec<Vec<[f32; 3]>> {
        self.ghosts.tracks.as_ref().map_or_else(Vec::new, |t| {
            t.heads()
                .map(|(_, s)| {
                    s.triangles
                        .iter()
                        .flat_map(|tr| tr.iter().map(|(p, _)| *p))
                        .collect()
                })
                .collect()
        })
    }

    /// Where each figure at a wheel holds its `steering` thread as the last frame posed it, by ghost.
    pub fn figure_steering(&self) -> Vec<(u32, f32)> {
        self.ghosts.player_steering()
    }

    /// The action each figure plays as the last frame posed it, by ghost.
    pub fn figure_actions(&self) -> Vec<(u32, String)> {
        self.ghosts.player_actions()
    }

    /// The tracks as the engine keeps them.
    pub fn tracks(&self) -> Option<&torque_render_scene::tracks::TireTracks> {
        self.ghosts.tracks.as_ref()
    }

    /// The frame's motion by `dt` seconds, with nothing drawn: the figure posed where the client runs it, the buggy's dust
    /// and tracks on `terrain`.
    pub fn update(
        &mut self,
        assets: &KitAssets,
        world: &mut TgeWorld,
        ghosts: &GhostPlayers,
        terrain: &Terrain,
        dt: f32,
    ) {
        let surfaces = |_: &_, at: [f32; 3], reach: f32| terrain.triangles_near(at, reach);
        let input = FrameInput {
            ghosts: Some(ghosts),
            fov: None,
            dt,
            surfaces: &surfaces,
        };
        self.ghosts.update(assets, world, &input);
    }

    /// The frame's objects: the figures and the buggy as [`WorldScene::update`] left them.
    pub fn objects(
        &mut self,
        view: &mut WorldView,
        assets: &KitAssets,
        world: &TgeWorld,
        eye: &Eye,
    ) -> Vec<DynamicObject> {
        let out = self.ghosts.objects(view, assets, world);
        self.ghosts.effects(view, assets, eye);
        out
    }
}
