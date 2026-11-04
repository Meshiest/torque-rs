//! The environment objects as a frame reads them: the Sky's distances and fog colour, and the Sun as GL light 0.
//!
//! - **Distances:** the Sky's visible and fog distances. The visible distance is the far plane (`GuiTSCtrl::onRender` passes the
//!   scene's visible distance to `dglSetFrustum`); without a Sky, the scene graph's defaults (500 and 250).
//! - **Fog colour:** the frame is cleared to it when there is no Sky (with one, [`crate::sky`] draws the fog-colour quad, the box,
//!   the clouds and the fog bans as TGE's Sky does). The fixed-function frame draws no distance fog of its own, so [`Fog`] stays
//!   off here.
//! - **Sun:** azimuth and elevation in degrees, then `MathUtils::getVectorFromAngles` (`(sin az cos el, cos az cos el, sin el)`
//!   towards the sun) and the direct and ambient colours as GL light 0's diffuse and ambient.
//!
//! Credit: follows TGE 1.4's `Sky` (`terrain/sky.cc`), `SceneGraph` (`sceneGraph/sceneGraph.cc`) and `MathUtils`
//! (`math/mathUtils.cc`), Copyright (C) GarageGames.com, Inc.; as Torque3D carries them under the MIT licence:
//! `scene/sceneManager.cpp` and `math/mathUtils.cpp`, Copyright (c) 2012 GarageGames, LLC. The code here is this crate's own.

use crate::source::SceneSource;
use crate::{Fog, Sun};

/// The distances when no Sky has been sent (the scene graph's defaults).
const DEFAULT_VISIBLE: f32 = 500.0;
const DEFAULT_FOG: f32 = 250.0;

/// The scene's visible and fog distances under a view distance cap `max` (the client's, [`SceneSource::max_view_distance`]): a
/// Sky whose visible distance is no more than the cap keeps its own distances; past it, the visible distance is the cap and the
/// fog distance the cap times the Sky's fog over its visible distance. The fog is scaled, not only the far plane: a cap of half
/// the Sky's halves both.
pub fn capped(visible: f32, fog: f32, max: f32) -> (f32, f32) {
    if visible <= max || visible <= 0.0 || !max.is_finite() {
        (visible, fog)
    } else {
        (max, max * (fog / visible))
    }
}

/// The Sky's visible distance as sent (no view distance cap: what a server scopes its ghosts by), None without a Sky or before
/// its distances arrive.
pub fn sky_visible(src: &dyn SceneSource) -> Option<f32> {
    src.sky()?.distances.map(|[v, _]| v)
}

/// What the environment objects give the frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Environment {
    pub visible_distance: f32,
    pub fog_distance: f32,
    pub clear: [f32; 3],
    pub sun: Sun,
    pub fog: Option<Fog>,
}

/// The material list (the `.dml` path) of the world's Sky as sent, None without a Sky or its main block.
pub fn sky_material(src: &dyn SceneSource) -> Option<String> {
    src.sky()?.main.map(|m| m.material)
}

impl Environment {
    pub fn of(src: &dyn SceneSource) -> Environment {
        let mut visible_distance = DEFAULT_VISIBLE;
        let mut fog_distance = DEFAULT_FOG;
        let mut clear = [1.0, 1.0, 1.0];
        if let Some(sky) = src.sky() {
            if let Some([v, f]) = sky.distances {
                visible_distance = v;
                fog_distance = f;
            }
            if let Some(main) = &sky.main {
                clear = main.fog_color;
            }
        }
        // The view distance's cap ([`SceneSource::max_view_distance`]): below the Sky's visible distance it takes its place and
        // the fog keeps its share of it ([`capped`]).
        if let Some(max) = src.max_view_distance() {
            (visible_distance, fog_distance) = capped(visible_distance, fog_distance, max);
        }
        let mut sun = Sun {
            towards: [0.0, 0.707, 0.707],
            diffuse: [0.0; 3],
            ambient: [0.0; 3],
        };
        if let Some(body) = src.sun() {
            let az = body.azimuth.to_radians();
            let el = body.elevation.to_radians();
            sun.towards = [az.sin() * el.cos(), az.cos() * el.cos(), el.sin()];
            sun.diffuse = body.direct;
            sun.ambient = body.ambient;
        }
        Environment {
            visible_distance,
            fog_distance,
            clear,
            sun,
            fog: None,
        }
    }
}

/// An empty frame of `src`'s camera at `size` with a horizontal field of view of `fov_deg` (the scene a 2D overlay is added to and
/// drawn with): the environment's clear and sun, no draws. None without a camera.
pub fn overlay_scene(
    src: &dyn SceneSource,
    fov_deg: f32,
    size: (u32, u32),
) -> Option<crate::Scene> {
    let eye = src.eye()?;
    let env = Environment::of(src);
    let camera =
        crate::camera::Camera::from_eye(eye, fov_deg, env.visible_distance, size.0, size.1);
    Some(crate::Scene {
        width: size.0,
        height: size.1,
        camera: camera.matrices(),
        clear: env.clear,
        sun: env.sun,
        fog: None,
        textures: Vec::new(),
        draws: Vec::new(),
        notes: Vec::new(),
    })
}

#[cfg(test)]
#[path = "env_tests.rs"]
mod tests;
