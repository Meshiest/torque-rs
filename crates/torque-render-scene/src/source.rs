//! What the renderers read of the world they draw: a [`SceneSource`] and the descriptions it hands out (the camera, the Sky,
//! the Sun and its flare). A client implements it over its own copy of the server's objects; the descriptions hold the values
//! as plain values (distances and colours as floats, names as text), whatever form the client keeps the objects in.

/// A camera to draw from: position, pitch and yaw (radians) and the field of view (the client's world's own).
pub use ghost_world::world::Eye;

/// A Sky cloud layer: its texture, height (0..100) and speed (0..10).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloudLayerDesc {
    pub texture: String,
    pub height: f32,
    pub speed: f32,
}

/// The Sky's main block: its material list, fog and sky colours, flags and cloud layers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SkyMainDesc {
    /// The material list's path (`.dml`), as sent.
    pub material: String,
    /// The fog colour as bytes (`mRealFogColor`) and as floats (0..1).
    pub fog_bytes: [u8; 3],
    pub fog_color: [f32; 3],
    /// The box's bottom face drawn; wind-driven precipitation; no fog bans.
    pub render_bottom: bool,
    pub wind_precipitation: bool,
    pub no_fog_bans: bool,
    /// `skyColor` (0..1).
    pub sky_color: [f32; 3],
    /// The fog volumes sent.
    pub fog_volumes: usize,
    pub clouds: [CloudLayerDesc; 3],
}

/// A Sky.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SkyDesc {
    pub main: Option<SkyMainDesc>,
    /// The visible and fog distances, when sent.
    pub distances: Option<[f32; 2]>,
    pub wind: Option<[f32; 3]>,
}

/// A Sun: azimuth and elevation (degrees, as sent) and its direct, ambient and shadow colours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SunDesc {
    pub azimuth: f32,
    pub elevation: f32,
    pub direct: [f32; 3],
    pub ambient: [f32; 3],
    pub shadow: [f32; 3],
}

/// How a sun flare blends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlareBlend {
    Alpha,
    One,
    Additive,
}

/// An `fxSunLight`: whether it is on, its two flare textures (local, remote), colour, brightness, size, blend and whether its
/// size follows its colour's luminance.
#[derive(Debug, Clone, PartialEq)]
pub struct FlareDesc {
    pub enabled: bool,
    pub textures: [String; 2],
    pub color: [u8; 3],
    pub brightness: f32,
    pub size: f32,
    pub blend: FlareBlend,
    pub linked: bool,
}

/// The world a frame is drawn from.
pub trait SceneSource {
    /// The camera, None before there is one.
    fn eye(&self) -> Option<Eye>;
    /// The horizontal field of view in degrees.
    fn fov_deg(&self) -> f32;
    /// A cap on the view distance the client set (None: the Sky's own).
    fn max_view_distance(&self) -> Option<f32>;
    /// The first Sky.
    fn sky(&self) -> Option<SkyDesc>;
    /// The first Sun with its colours sent.
    fn sun(&self) -> Option<SunDesc>;
    /// The first sun flare with its fields sent.
    fn sun_flare(&self) -> Option<FlareDesc>;
    /// Whether the world holds a map's objects (terrain, interiors, water, statics or a mission area).
    fn has_map(&self) -> bool;
}
