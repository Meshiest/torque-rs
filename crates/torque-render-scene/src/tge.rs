//! TGE's own set as a frame's source: [`SceneSource`] and [`HdSource`] (with every TGE default) over a record world of TGE's
//! classes ([`TgeClientSet`]): the first Sky's init block and distances, and the first Sun's update.
//!
//! - **Sky:** the material list, the fog colour (floats; as bytes, `ColorI` of them), `renderBottomTexture`, `noRenderBans`,
//!   `effectPrecipitation`, the fog volumes, the cloud layers (texture, height, speed) and the wind; a Sky with its textures off
//!   (`useSkyTextures` 0) is drawn in its `SkySolidColor` ([`crate::sky::solid`]: no material list, the solid colour as the sky
//!   colour); else the sky colour is white. The visible and fog distances as sent.
//! - **Sun:** `Sun::packUpdate` sends its light's direction (from the sun: the negated `getVectorFromAngles` of its azimuth and
//!   elevation), colour and ambient; the angles are taken back from the direction. TGE's Sun has no shadow colour (black).
//!
//! Credit: follows TGE 1.4's `terrain/sky.cc` and `terrain/sun.cc`, Copyright (C) GarageGames.com, Inc.; the code here is this
//! crate's own.

use ghost_world::tge::TgeClientSet;
use ghost_world::world::ClientWorld;
use ghosts::{TgeClass, TgeRecord};

use crate::hd::sync::HdSource;
use crate::source::{CloudLayerDesc, Eye, FlareDesc, SceneSource, SkyDesc, SkyMainDesc, SunDesc};

/// The first record of `class` in `w` that `pick` takes.
fn first<'w, T>(
    w: &'w ClientWorld<TgeClientSet>,
    class: TgeClass,
    pick: impl Fn(&'w TgeRecord) -> Option<T>,
) -> Option<T> {
    w.of_class(class)
        .find_map(|(_, g)| g.record.as_ref().and_then(&pick))
}

/// The azimuth and elevation (degrees, azimuth in 0..360) of the direction `towards` (towards the sun).
pub fn angles_of(towards: [f32; 3]) -> (f32, f32) {
    let [x, y, z] = towards;
    let az = x.atan2(y).to_degrees().rem_euclid(360.0);
    let el = z.clamp(-1.0, 1.0).asin().to_degrees();
    (az, el)
}

impl SceneSource for ClientWorld<TgeClientSet> {
    fn eye(&self) -> Option<Eye> {
        ClientWorld::eye(self)
    }

    fn fov_deg(&self) -> f32 {
        ClientWorld::fov_deg(self)
    }

    fn max_view_distance(&self) -> Option<f32> {
        self.max_view_distance
    }

    fn sky(&self) -> Option<SkyDesc> {
        let sky = first(self, TgeClass::Sky, |r| match r {
            TgeRecord::Sky(s) => Some(s),
            _ => None,
        })?;
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let main = sky.init.as_ref().map(|m| {
            let fog_color = m.fog_color.map(|c| c.get());
            let textures_on = m.textures_on != 0;
            SkyMainDesc {
                material: if textures_on {
                    text(&m.material_list)
                } else {
                    String::new()
                },
                fog_bytes: fog_color.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8),
                fog_color,
                render_bottom: m.render_box_bottom != 0,
                wind_precipitation: m.effect_precipitation != 0,
                no_fog_bans: m.no_render_bans,
                sky_color: if textures_on {
                    [1.0; 3]
                } else {
                    m.solid_fill_color.map(|c| c.get())
                },
                fog_volumes: m.fog_volumes.len(),
                clouds: [0, 1, 2].map(|i| CloudLayerDesc {
                    texture: text(&m.clouds[i].texture),
                    height: m.clouds[i].height.get(),
                    speed: m.clouds[i].speed.get(),
                }),
            }
        });
        Some(SkyDesc {
            distances: sky.visibility.map(|d| d.map(|v| v.get())),
            wind: sky
                .wind
                .or(sky.init.as_ref().map(|m| m.wind))
                .map(|w| w.map(|v| v.get())),
            main,
        })
    }

    fn sun(&self) -> Option<SunDesc> {
        let u = first(self, TgeClass::Sun, |r| match r {
            TgeRecord::Sun(s) => s.update.as_ref(),
            _ => None,
        })?;
        let d = u.direction.map(|v| v.get());
        let (azimuth, elevation) = angles_of([-d[0], -d[1], -d[2]]);
        let rgb = |c: [ghosts::F32; 4]| [c[0].get(), c[1].get(), c[2].get()];
        Some(SunDesc {
            azimuth,
            elevation,
            direct: rgb(u.color),
            ambient: rgb(u.ambient),
            shadow: [0.0; 3],
        })
    }

    /// TGE's `fxSunLight` is not one of the classes TGE's set ghosts.
    fn sun_flare(&self) -> Option<FlareDesc> {
        None
    }

    /// TGE's set ghosts none of a map's own classes (terrain, interiors, water).
    fn has_map(&self) -> bool {
        false
    }
}

impl HdSource for ClientWorld<TgeClientSet> {}

#[cfg(test)]
#[path = "tge_tests.rs"]
pub(crate) mod tests;
