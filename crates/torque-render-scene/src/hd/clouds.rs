//! The volumetric clouds as plain data (what
//! the server's Sky says about its clouds,
//! the layers they live in, the quality ladder, and the noise the GPU marches through (generated here at load, deterministic, no
//! asset).
//!
//! - **The layers** ([`FAIR_WEATHER`], a small list of [`CloudLayerDef`]: presets are a different list): the low cumulus deck
//!   ([`CUMULUS`], [`SLAB`]: a 2.5D slab about a kilometre up, a person being about 2.6 units tall), a mid altocumulus layer
//!   ([`ALTOCUMULUS`]: a thin, patchy sheet of small rounded elements in rows, about 4 km up) and high cirrus ([`CIRRUS`]: a 2D
//!   sheet about 9 km up with its own fibred noise, [`cirrus_plane`]). Each has its own height, thickness, coverage (a share of the
//!   sky's), medium, wind (the sky's turned and sped up: [`layer_drift`]) and light, and a quality draws some of them
//!   ([`CloudQuality::layers`]: Medium the deck, High the deck and the cirrus, Ultra all three). They are anchored to the world (the
//!   clouds pass overhead as the camera moves); the march starts at the eye wherever it is (a camera in a layer is in its cloud, one
//!   over a layer sees it below, lit from above). Each layer takes the sun through the air at its own height ([`UNITS_PER_KM`]): at dusk the
//!   low deck is in the earth's shadow while the high layers still glow. A higher layer shades a lower one (its coverage where the
//!   sun's ray crosses it, [`CloudLight::layer_shadow`]); a lower deck's sunlit tops light the undersides above it. With the
//!   physical sky drawn in the box's place the air between the eye and a cloud takes it toward the sky behind it (aerial perspective, each layer's
//!   [`CloudLayerDef::aerial`]) and a low ray's long slant through a slab thins its coverage ([`CloudLight::thin_slant`]: no wall
//!   along the horizon); with the server's box far clouds fade into it over [`CloudSlab::haze`] units. All fade out before their
//!   reach (their flat bases would show edge-on at the horizon).
//! - **The server's settings** ([`CloudSky::of_world`]): the Sky's cloud layers (TGE's `cloudText`, `cloudHeightPer`, `cloudSpeed`)
//!   and `windVelocity`. Their coverage is the layers' combined opacity (each texture's mean alpha), their colour the alpha-weighted
//!   mean of the textures (a storm's grey clouds stay grey); with no layers a fair-weather default. The
//!   wind gives the drift; with none a gentle default breeze. The ambient light is the server's own sky: the
//!   mean colour of its box above (the clouds' tops) and at the horizon (their undersides), so a dusk sky's clouds take some of its
//!   colours ([`OVERCAST`]: half of them; under an overcast none, and the sun's light and the tint none either: a closed deck is
//!   grey-white). A scene without a server sky (a synthetic check scene) has no clouds.
//! - **The look** ([`CloudLook`]): a scene may name its own layers, light and coverage, the sunlight through the air in place of
//!   the ground's sun, the clouds' shadows on the ground at every quality and a march refined through the clouds
//!   ([`CloudLook::SUNLIT_CUMULUS`]); without one the engine's ([`CloudLook::DEFAULT`]), drawn as before.
//! - **The noise** ([`noise`]): our own tiling gradient (Perlin-style) and cell (Worley-style) noise, every octave at least four
//!   texels a cell (finer ones only alias). The shape volume (128³ RGBA8) holds the Perlin-Worley base in r and Worley fBm at three
//!   rising frequencies in gba; the detail volume (64³) Worley fBm at three frequencies; the weather map (256², 2D) the coverage's
//!   variation over the sky (r), the clouds' kind (g: how tall they grow), where the altocumulus lies in rows (b: clear patches, rows,
//!   loose cells) and the rows' slow bend (a); the cirrus plane (512², 2D) its fibres, patches and a second set of fibres; the blue
//!   noise (64², void and cluster after Ulichney 1993) offsets each texel's march, rotated each frame by the golden ratio. Every
//!   volume tiles; the periods are not multiples of one another and each layer's domain is turned off the world's axes, so no
//!   repetition lines up within the haze's reach.
//!
//! **In the sky**: drawn after the sun disk and the server's flat cloud layers (kept: the sky's high layer; a storm's
//! overcast is its layers) and before the fog bans; the frame's fog toward the sky behind a pixel sees them without their detail;
//! the sun's light in the air (the god rays) and at Ultra on the ground goes through their shadow map; a surface the eye sees through
//! a layer's slab has them laid over it.
//!
//! The lighting (WGSL, `clouds.wgsl`): Beer's law toward the sun in multiple-scattering octaves (Wrenninge 2013, "Oz: the great
//! and volumetric": each octave's energy, extinction and phase eccentricity a share of the last's) of a dual-lobe Henyey-Greenstein
//! phase, a powder term away from the sun, the sun's light through the atmosphere (warm at a low sun), the ambient from the sky
//! (the physical sky in the box's place: the atmosphere's; the server's box: its colours) dimmed in the clouds' lower parts, integrated per step as
//! Hillaire 2016 does ("Physically based sky, atmosphere and cloud rendering in Frostbite"). A low sun's long path through the deck
//! shades less past the cloud's own steps when the eye looks at its sunlit side ([`CloudLight::far_shadow`]). The cirrus is one
//! scattering in thin ice. The history is held within the march's own neighbourhood and read with a Catmull-Rom filter (no
//! smearing, no blur that grows frame by frame).

use std::sync::OnceLock;

use crate::source::SceneSource;

use crate::hd::{HdDraw, Quality, TextureTable, srgb_to_linear};

/// The cloud slab and how far it is seen (world units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudSlab {
    pub base: f32,
    pub top: f32,
    /// Under the server's box far clouds keep `exp(-(d / haze)^2)` of themselves over the box behind (the haze grows with a layer's
    /// reach).
    pub haze: f32,
    /// The farthest the march goes along a ray.
    pub reach: f32,
    /// Where every cloud fades out (from, to: units along the ray), before the reach.
    pub fade: (f32, f32),
}

/// The low cumulus deck's slab (the clouds' shadows on the ground are cast from its base).
pub const SLAB: CloudSlab = CloudSlab {
    base: 1500.0,
    top: 2700.0,
    haze: 26000.0,
    reach: 60000.0,
    fade: (34000.0, 58000.0),
};

/// The most the layers are raised over a map ([`CloudSky::lift`]).
pub const MAX_LIFT: f32 = 20_000.0;

/// World units in a kilometre of the atmosphere (a person is about 2.6 units, 1.8 m: the deck's base is about a kilometre up), for
/// the sun's light through the air at each layer's height.
pub const UNITS_PER_KM: f32 = 1500.0;

/// How a cloud layer is drawn: marched through a slab, or a thin sheet looked up where the ray crosses its height (its own noise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerForm {
    Volume,
    Sheet,
}

/// The density's height profile of a marched layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerProfile {
    /// Cumulus: a soft flat base and rounded tops whose height the weather map's kind sets (low heaps to tall towers).
    Heaps,
    /// A thin sheet of small rounded elements (altocumulus): rounded above and below, the shape's cells its elements.
    Elements,
}

/// One cloud layer: where it is, how much of the sky it covers, its medium, its wind and its light. A sheet's `top` is its `base`,
/// its `extinction` its optical depth straight up at density 1, its `shape_period` the fibres' period and `weather_period` the
/// patches'.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudLayerDef {
    pub name: &'static str,
    pub form: LayerForm,
    pub profile: LayerProfile,
    pub base: f32,
    pub top: f32,
    /// Its coverage: the sky's (the server's layers or the fair-weather default) times `coverage`, plus `coverage_add`.
    pub coverage: f32,
    pub coverage_add: f32,
    pub extinction: f32,
    pub shape_period: f32,
    pub detail_period: f32,
    pub weather_period: f32,
    /// The coverage's variation over the sky (the weather map's share) and the detail's erosion.
    pub variation: f32,
    pub detail: f32,
    /// The noise domain's turn off the world's axes (radians), the wind's turn from the sky's (radians, counter-clockwise seen from
    /// above) and its speed as a multiple of the sky's.
    pub turn: f32,
    pub wind_turn: f32,
    pub wind_speed: f32,
    /// The edges' cut at the base and toward the tops, the tops' extra density.
    pub edge_base: f32,
    pub edge_top: f32,
    pub top_density: f32,
    /// The aerial perspective's distance (units; the air thins with height, so a higher layer's is longer), the far fade (from, to:
    /// units along the ray) and the reach.
    pub aerial: f32,
    pub fade: (f32, f32),
    pub reach: f32,
    /// The sun's and the ambient's gains.
    pub sun: f32,
    pub ambient: f32,
    /// The march's steps as a share of the quality's and the light march's most steps (0: the quality's); a sheet has neither.
    pub steps: f32,
    pub light_steps: u32,
    /// The slow change of shape (units a second through the volume's third axis).
    pub evolve: f32,
    /// How many mip levels blurrier than the pixel's footprint its noise is read (a far, fine layer's smallest features held to a few
    /// of the march's texels: no grain).
    pub blur: f32,
}

/// The low cumulus deck: big, puffy fair-weather cumulus.
pub const CUMULUS: CloudLayerDef = CloudLayerDef {
    name: "cumulus",
    form: LayerForm::Volume,
    profile: LayerProfile::Heaps,
    base: SLAB.base,
    top: SLAB.top,
    coverage: 0.98,
    coverage_add: 0.0,
    extinction: 0.065,
    shape_period: SHAPE_PERIOD,
    detail_period: DETAIL_PERIOD,
    weather_period: WEATHER_PERIOD,
    variation: 0.7,
    detail: 0.4,
    turn: DOMAIN_TURN,
    wind_turn: 0.0,
    wind_speed: 1.0,
    edge_base: 0.02,
    edge_top: 0.11,
    top_density: 0.6,
    aerial: 24000.0,
    fade: SLAB.fade,
    reach: SLAB.reach,
    sun: 1.5,
    ambient: 0.75,
    steps: 1.0,
    light_steps: 0,
    evolve: 0.7,
    blur: 0.0,
};

/// The mid layer: patchy altocumulus, sheets of small rounded elements about 4 km up, in a wind veered from the deck's and faster.
pub const ALTOCUMULUS: CloudLayerDef = CloudLayerDef {
    name: "altocumulus",
    form: LayerForm::Volume,
    profile: LayerProfile::Elements,
    base: 6000.0,
    top: 6500.0,
    coverage: 0.95,
    coverage_add: 0.15,
    extinction: 0.05,
    shape_period: 2400.0,
    detail_period: 700.0,
    weather_period: 21000.0,
    variation: 1.5,
    detail: 0.1,
    turn: 1.13,
    wind_turn: 0.45,
    wind_speed: 2.2,
    edge_base: 0.0,
    edge_top: 0.0,
    top_density: 0.3,
    aerial: 55000.0,
    fade: (60000.0, 110000.0),
    reach: 115000.0,
    sun: 0.4,
    ambient: 0.45,
    steps: 0.2,
    light_steps: 4,
    evolve: 1.2,
    blur: 2.6,
};

/// The high layer: thin, wispy cirrus about 9 km up, a sheet with its own fibred noise, in a fast wind veered further.
pub const CIRRUS: CloudLayerDef = CloudLayerDef {
    name: "cirrus",
    form: LayerForm::Sheet,
    profile: LayerProfile::Heaps,
    base: 13500.0,
    top: 13500.0,
    coverage: 1.0,
    coverage_add: 0.05,
    extinction: 0.55,
    shape_period: CIRRUS_PERIOD,
    detail_period: CIRRUS_PERIOD * 0.5,
    weather_period: 61000.0,
    variation: 0.8,
    detail: 0.2,
    turn: 0.0,
    wind_turn: 0.8,
    wind_speed: 4.5,
    edge_base: 0.0,
    edge_top: 0.0,
    top_density: 0.0,
    aerial: 80000.0,
    fade: (300000.0, 600000.0),
    reach: 600000.0,
    sun: 1.6,
    ambient: 0.9,
    steps: 0.0,
    light_steps: 0,
    evolve: 0.0,
    blur: 1.6,
};

/// The fair-weather set: the layers by how much they matter (a quality draws some of them, [`CloudQuality::layers`]); they are drawn
/// from the lowest up.
pub const FAIR_WEATHER: [CloudLayerDef; 3] = [CUMULUS, CIRRUS, ALTOCUMULUS];

/// The most layers the GPU takes.
pub const MAX_LAYERS: usize = 3;

/// The cirrus sheet's noise (2D, RGBA8: r its fibres, g its patches, b a second set of fibres): texels a side and its period (units).
pub const CIRRUS_SIZE: usize = 512;
pub const CIRRUS_PERIOD: f32 = 60000.0;

/// A layer's drift over the ground: the sky's turned by the layer's wind and scaled by its speed.
pub fn layer_drift(sky: [f32; 2], layer: &CloudLayerDef) -> [f32; 2] {
    let (c, s) = (layer.wind_turn.cos(), layer.wind_turn.sin());
    [
        (c * sky[0] - s * sky[1]) * layer.wind_speed,
        (s * sky[0] + c * sky[1]) * layer.wind_speed,
    ]
}

/// A layer's coverage under a sky of `coverage`.
pub fn layer_coverage(coverage: f32, layer: &CloudLayerDef) -> f32 {
    (coverage * layer.coverage + layer.coverage_add).clamp(0.0, 0.95)
}

/// The layers of `set` a mask draws (bit i: `set[i]`), lowest first, at most [`MAX_LAYERS`].
pub fn drawn_layers(set: &[CloudLayerDef], mask: u32) -> Vec<CloudLayerDef> {
    let mut v: Vec<CloudLayerDef> = set
        .iter()
        .enumerate()
        .filter(|(i, _)| *i < 32 && mask & (1 << i) != 0)
        .map(|(_, l)| *l)
        .take(MAX_LAYERS)
        .collect();
    v.sort_by(|a, b| a.base.total_cmp(&b.base));
    v
}

/// The noise volumes' periods in world units and the domain's turn off the world's axes (radians).
pub const SHAPE_PERIOD: f32 = 4300.0;
pub const DETAIL_PERIOD: f32 = 610.0;
pub const WEATHER_PERIOD: f32 = 29000.0;
pub const DOMAIN_TURN: f32 = 0.41;
/// The noise volumes' sizes (texels a side).
pub const SHAPE_SIZE: usize = 128;
pub const DETAIL_SIZE: usize = 64;
pub const WEATHER_SIZE: usize = 256;
pub const BLUE_SIZE: usize = 64;
/// The seed every volume is generated from.
pub const SEED: u32 = 0x0b1c_c10d;

/// The cloud shadow map (the sun's transmittance through the slab on the base's plane, around the eye): texels a side and the
/// least it spans (units; else twice the visible distance and a margin).
pub const SHADOW_TEXELS: u32 = 256;
pub const SHADOW_MIN_SPAN: f32 = 640.0;
/// The shadow map's march through the slab toward the sun (steps).
pub const SHADOW_STEPS: u32 = 16;

/// The sun's light through the clouds as the eye sees it (the clouds pass's `fs_sun_rays` and `fs_sun_mean`): the mean
/// transmittance of [`SUN_RAYS`] rays from the eye across a disc of `spread` radians about the sun, each layer's segment by the
/// march's own rules (its spread, its thinning, its far fade) in [`SUN_SEGMENTS`] parts of [`SUN_SEGMENT_STEPS`] steps. Where the
/// cloud shadow map holds nothing (a sun under `low.0` in sine of its elevation) the sun's light on every surface, the
/// volumetric light and the atmosphere's glow about the sun take it; from `low.0` to `low.1` it hands over to the map. At a low sun
/// the ground's ray to the sun meets the deck tens of kilometres off, and that ray and the eye's toward the sun cross the same clouds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudSun {
    /// The share of the light a full cover takes away (1: all of it, as the disk the eye sees behind the clouds).
    pub strength: f32,
    /// The sine of the sun's elevation over which the eye's value hands over to the shadow map (wholly the eye's under the first).
    pub low: (f32, f32),
    /// The disc the rays cover about the sun (radians): the sun's light fades as a cloud's edge crosses it, not at one ray.
    pub spread: f32,
    /// How far from the sun the atmosphere's glow takes it (radians; all of it at the sun, none past this).
    pub aureole: f32,
}

pub const CLOUD_SUN: CloudSun = CloudSun {
    strength: 1.0,
    low: (0.05, 0.12),
    spread: 0.03,
    aureole: 0.5,
};
/// The rays toward the sun, the parts each layer's segment is marched in (each part by its own invocation) and the steps a part.
pub const SUN_RAYS: u32 = 16;
pub const SUN_SEGMENTS: u32 = 4;
pub const SUN_SEGMENT_STEPS: u32 = 8;

/// The clouds' light (every layer's): the phase's lobes, the multiple-scattering octaves, the diffuse share, the powder's strength,
/// the light march's first step (units, doubling each step), the ambient's floor, the sun's tint and the undersides' light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudLight {
    pub forward_g: f32,
    pub back_g: f32,
    pub back_share: f32,
    /// The multiple-scattering octaves: how many, and each one's energy, extinction and phase eccentricity as a share of the
    /// last's.
    pub octaves: u32,
    pub octave_energy: f32,
    pub octave_extinction: f32,
    pub octave_eccentricity: f32,
    /// The diffuse multiple scattering: its share and how much slower than the direct light it decays.
    pub multiple: f32,
    pub multiple_decay: f32,
    /// The ambient's share left at the clouds' bases (the cloud above them shades them).
    pub ambient_floor: f32,
    /// How much of the atmosphere's colour toward the sun the sun's light on the clouds takes (1: all; a low sun's peach).
    pub sun_tint: f32,
    pub powder: f32,
    pub light_step: f32,
    /// The sunlit ground's light on the clouds' undersides (a share of the sun's light times its height's sine, and of the sky's
    /// light above), and the share of the horizon's colour they see beside it.
    pub bounce: f32,
    pub underside: f32,
    /// How much a higher layer's shadow takes from the sun's light on a lower one (its optical depth's share: the light it
    /// scatters forward still arrives), and the most the sun's slant lengthens that path.
    pub layer_shadow: f32,
    pub layer_slant: f32,
    /// The light march's far shadow: past its first `near_steps` steps (the cloud's own shape), the share of the optical depth a sun
    /// on the horizon keeps (all of it overhead; a low sun's long path through the deck would put every cloud in another's shadow).
    pub far_shadow: f32,
    pub near_steps: u32,
    /// The cosines to the sun over which a view toward it takes that easing back (from, to): looking at the light through a cloud,
    /// its whole path shades it (dark cores, bright rims); looking at its sunlit side, the near steps shape it.
    pub far_view: (f32, f32),
    /// The slant (a ray's path through a volume layer over its thickness) past which the layer thins out along that ray: its
    /// coverage spreads (each cloud on the ray takes `1 - (1 - c)^e` of the sky, `e = (thin_slant / slant)^thin_power`: together the
    /// many clouds a low ray crosses cover about what one column overhead does) and its density thins (times
    /// `(thin_slant / slant)^thin_density`). A low ray would otherwise pile many clouds into a wall near the horizon and the deck
    /// would read as a dome.
    pub thin_slant: f32,
    pub thin_power: f32,
    pub thin_density: f32,
}

pub const CLOUD_LIGHT: CloudLight = CloudLight {
    forward_g: 0.78,
    back_g: -0.25,
    back_share: 0.3,
    octaves: 3,
    octave_energy: 0.5,
    octave_extinction: 0.25,
    octave_eccentricity: 0.55,
    multiple: 3.0,
    multiple_decay: 0.15,
    ambient_floor: 0.4,
    sun_tint: 1.0,
    powder: 0.35,
    light_step: 15.0,
    bounce: 0.3,
    underside: 0.45,
    layer_shadow: 0.1,
    layer_slant: 1.5,
    far_shadow: 0.3,
    near_steps: 4,
    far_view: (-0.2, 1.5),
    thin_slant: 3.0,
    thin_power: 0.35,
    thin_density: 0.0,
};

/// What the clouds are drawn with at a quality: the march's reduced size (internal pixels a texel, a side), its steps through the
/// deck, the light march's steps toward the sun, the detail erosion, the clouds' shadows on the ground, the history's weight and
/// how far it may stray from the march's neighbourhood (the box's half width in its standard deviations), and the layers drawn (bit
/// i: [`FAIR_WEATHER`]`[i]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudQuality {
    pub scale: u32,
    pub steps: u32,
    pub light_steps: u32,
    pub detail: bool,
    pub ground_shadows: bool,
    pub history: f32,
    pub spread: f32,
    pub layers: u32,
}

impl CloudQuality {
    /// The clouds at `quality`: none at Low; Medium a cheaper pass at a sixteenth of the pixels, 32 steps, no detail, the
    /// cumulus deck only; High a quarter of the pixels, 48 steps, the cirrus over the deck; Ultra 96 steps, all three layers and
    /// the clouds' shadows on the ground ([`crate::hd::ImageSettings::clouds`] decides whether they are drawn).
    pub fn of(quality: Quality) -> Option<CloudQuality> {
        let q = match quality {
            Quality::Low => return None,
            Quality::Medium => CloudQuality {
                scale: 4,
                steps: 32,
                light_steps: 3,
                detail: false,
                ground_shadows: false,
                history: 0.88,
                spread: 1.5,
                layers: 0b001,
            },
            Quality::High => CloudQuality {
                scale: 2,
                steps: 48,
                light_steps: 5,
                detail: true,
                ground_shadows: false,
                history: 0.9,
                spread: 1.25,
                layers: 0b011,
            },
            Quality::Ultra => CloudQuality {
                scale: 2,
                steps: 96,
                light_steps: 5,
                detail: true,
                ground_shadows: true,
                history: 0.9,
                spread: 1.25,
                layers: 0b111,
            },
        };
        Some(q)
    }
}

/// How the volumetric clouds look: the layers in [`FAIR_WEATHER`]'s place (the same order: a quality's mask picks from them), their
/// light, the sky's coverage in place of the server's (None: the server's or the fair-weather default) and the clouds' shadows on
/// the ground at every quality that draws them (the share of the sun a full cloud takes away; None: only at Ultra, the engine's
/// share). With `sun` the clouds take that sunlight (linear) through the air at each layer's height, dimmed and reddened as the
/// atmosphere's transmittance has it (a sunset's clouds glow orange); without it, or under the moon, the scene's sun's light, its
/// color taken from the air but its strength the ground's. With a `fine_step` the march is refined: through clear air in the
/// quality's steps, through cloud in steps of that many units (no shorter than a pixel there), at most `fine_budget` times the
/// quality's samples a layer; the even march's first sample in a cloud lies a long step deep, where the sun's light has gone, so
/// the sunlit shell that gives a cumulus its lit side, its dark side and its bright rim is only found by the fine steps.
///
/// A scene names a look ([`crate::hd::HdScene::cloud_look`]); without one the engine's ([`CloudLook::DEFAULT`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudLook {
    pub layers: [CloudLayerDef; MAX_LAYERS],
    pub light: CloudLight,
    pub coverage: Option<f32>,
    pub ground_shadow: Option<f32>,
    pub sun: Option<[f32; 3]>,
    pub fine_step: f32,
    pub fine_budget: f32,
}

impl CloudLook {
    /// The engine's clouds: the fair-weather layers and their light, the scene's sun, the even march.
    pub const DEFAULT: CloudLook = CloudLook {
        layers: FAIR_WEATHER,
        light: CLOUD_LIGHT,
        coverage: None,
        ground_shadow: None,
        sun: None,
        fine_step: 0.0,
        fine_budget: 1.0,
    };

    /// Sunlit fair-weather cumulus: a deeper deck (to 2 km) whose heads the sun lights bright white over gray-blue bases (a
    /// strong sun on them, little of the sky's light, the powder whole, the light scattered inside kept near the sunlit
    /// surface), the march refined through the clouds, the sunlight through the air (golden, then orange and red at sunset),
    /// the cirrus thinner and the clouds' shadows on the ground at every quality.
    pub const SUNLIT_CUMULUS: CloudLook = CloudLook {
        layers: [
            CloudLayerDef {
                top: 3000.0,
                edge_top: 0.13,
                sun: 8.0,
                ambient: 0.4,
                ..CUMULUS
            },
            CloudLayerDef {
                coverage: 0.7,
                ..CIRRUS
            },
            ALTOCUMULUS,
        ],
        light: CloudLight {
            powder: 1.0,
            multiple: 2.0,
            multiple_decay: 0.6,
            ambient_floor: 0.1,
            ..CLOUD_LIGHT
        },
        coverage: None,
        ground_shadow: Some(0.6),
        sun: Some([1.0, 0.96, 0.9]),
        fine_step: 30.0,
        fine_budget: 3.0,
    };

    /// The most samples a layer the refined march takes, as a multiple of the quality's.
    pub const MAX_FINE_BUDGET: f32 = 8.0;

    /// This look with every value the renderer can take: non-finite numbers the engine's (or none), the coverage within the
    /// server's range (0 to [`MAX_COVERAGE`]), the ground's shadow 0 to 1, the sunlight not negative, the fine step not negative
    /// and its budget 1 to [`CloudLook::MAX_FINE_BUDGET`], each volume layer at least [`MIN_THICKNESS`] thick (a top under its
    /// base swapped with it), a sheet's top its base, the noise periods positive, a layer's steps at most [`MAX_STEP_SHARE`] of
    /// the quality's and the octaves at most [`MAX_OCTAVES`] (the march's loops stay bounded). The engine's looks are unchanged by it.
    pub fn sanitized(&self) -> CloudLook {
        let finite = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let positive = |v: f32, d: f32| if v.is_finite() && v > 0.0 { v } else { d };
        let mut layers = self.layers;
        for (l, d) in layers.iter_mut().zip(&FAIR_WEATHER) {
            let (a, b) = (finite(l.base, d.base), finite(l.top, d.top));
            l.base = a.min(b);
            l.top = if l.form == LayerForm::Sheet {
                l.base
            } else {
                a.max(b).max(l.base + MIN_THICKNESS)
            };
            l.coverage = finite(l.coverage, d.coverage).max(0.0);
            l.coverage_add = finite(l.coverage_add, d.coverage_add);
            l.extinction = finite(l.extinction, d.extinction).max(0.0);
            l.shape_period = positive(l.shape_period, d.shape_period);
            l.detail_period = positive(l.detail_period, d.detail_period);
            l.weather_period = positive(l.weather_period, d.weather_period);
            l.variation = finite(l.variation, d.variation);
            l.detail = finite(l.detail, d.detail);
            l.turn = finite(l.turn, d.turn);
            l.wind_turn = finite(l.wind_turn, d.wind_turn);
            l.wind_speed = finite(l.wind_speed, d.wind_speed);
            l.edge_base = finite(l.edge_base, d.edge_base);
            l.edge_top = finite(l.edge_top, d.edge_top);
            l.top_density = finite(l.top_density, d.top_density);
            l.aerial = positive(l.aerial, d.aerial);
            let (f0, f1) = (finite(l.fade.0, d.fade.0), finite(l.fade.1, d.fade.1));
            l.fade = (f0.min(f1), f0.max(f1));
            l.reach = finite(l.reach, d.reach).max(0.0);
            l.sun = finite(l.sun, d.sun).max(0.0);
            l.ambient = finite(l.ambient, d.ambient).max(0.0);
            l.steps = finite(l.steps, d.steps).clamp(0.0, MAX_STEP_SHARE);
            l.evolve = finite(l.evolve, d.evolve);
            l.blur = finite(l.blur, d.blur);
        }
        let (l, d) = (self.light, CLOUD_LIGHT);
        let light = CloudLight {
            forward_g: finite(l.forward_g, d.forward_g).clamp(-0.99, 0.99),
            back_g: finite(l.back_g, d.back_g).clamp(-0.99, 0.99),
            back_share: finite(l.back_share, d.back_share),
            octaves: l.octaves.min(MAX_OCTAVES),
            octave_energy: finite(l.octave_energy, d.octave_energy),
            octave_extinction: finite(l.octave_extinction, d.octave_extinction),
            octave_eccentricity: finite(l.octave_eccentricity, d.octave_eccentricity),
            multiple: finite(l.multiple, d.multiple),
            multiple_decay: finite(l.multiple_decay, d.multiple_decay),
            ambient_floor: finite(l.ambient_floor, d.ambient_floor),
            sun_tint: finite(l.sun_tint, d.sun_tint),
            powder: finite(l.powder, d.powder),
            light_step: positive(l.light_step, d.light_step),
            bounce: finite(l.bounce, d.bounce),
            underside: finite(l.underside, d.underside),
            layer_shadow: finite(l.layer_shadow, d.layer_shadow),
            layer_slant: positive(l.layer_slant, d.layer_slant),
            far_shadow: finite(l.far_shadow, d.far_shadow),
            near_steps: l.near_steps.min(MAX_OCTAVES * 2),
            far_view: (
                finite(l.far_view.0, d.far_view.0),
                finite(l.far_view.1, d.far_view.1),
            ),
            thin_slant: positive(l.thin_slant, d.thin_slant),
            thin_power: finite(l.thin_power, d.thin_power),
            thin_density: finite(l.thin_density, d.thin_density),
        };
        CloudLook {
            layers,
            light,
            coverage: self
                .coverage
                .filter(|c| c.is_finite())
                .map(|c| c.clamp(0.0, MAX_COVERAGE)),
            ground_shadow: self
                .ground_shadow
                .filter(|s| s.is_finite())
                .map(|s| s.clamp(0.0, 1.0)),
            sun: self.sun.map(|c| c.map(|v| finite(v, 0.0).max(0.0))),
            fine_step: finite(self.fine_step, 0.0).max(0.0),
            fine_budget: finite(self.fine_budget, 1.0).clamp(1.0, Self::MAX_FINE_BUDGET),
        }
    }

    /// `q` as this look asks for it (the ground's shadows when it casts them).
    pub fn quality(&self, q: CloudQuality) -> CloudQuality {
        CloudQuality {
            ground_shadows: self.ground_shadow.map_or(q.ground_shadows, |s| s > 0.0),
            ..q
        }
    }
}

/// An overcast's light: as the sky's coverage rises from `from` to `to`, the clouds' light turns neutral (the ambient, the sun's
/// light on them and their tint lose their colour, their brightness kept): under a closed deck the light is the sun's scattered
/// through the clouds, grey-white, not the blue sky's or a server box's tint. And a server box's colours lend the clouds' ambient
/// only `box_chroma` of their colour at any coverage (a box is painted, not the air's light: a lavender box made lavender clouds);
/// with our own sky the atmosphere's colours stay (the blue-violet of a low sun's sky).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overcast {
    pub from: f32,
    pub to: f32,
    pub box_chroma: f32,
}

pub const OVERCAST: Overcast = Overcast {
    from: 0.5,
    to: 0.65,
    box_chroma: 0.5,
};

/// How overcast a sky of `coverage` is (0 fair weather, 1 closed).
pub fn overcast_share(coverage: f32) -> f32 {
    let t = ((coverage - OVERCAST.from) / (OVERCAST.to - OVERCAST.from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A linear colour with `keep` of its chroma (its Rec. 709 luminance kept).
pub fn keep_chroma(c: [f32; 3], keep: f32) -> [f32; 3] {
    let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    c.map(|v| l + (v - l) * keep.clamp(0.0, 1.0))
}

/// The most coverage the server's layers or a look give the sky.
pub const MAX_COVERAGE: f32 = 0.92;
/// The thinnest a volume layer is (units), the most of the quality's steps a layer of a look takes and the most
/// multiple-scattering octaves.
pub const MIN_THICKNESS: f32 = 1.0;
pub const MAX_STEP_SHARE: f32 = 4.0;
pub const MAX_OCTAVES: u32 = 8;

// The coverage with no cloud layers (a fair-weather sky), and how the layers' combined opacity maps onto it.
pub const DEFAULT_COVERAGE: f32 = 0.47;
pub const LAYER_COVERAGE_FLOOR: f32 = 0.3;
pub const LAYER_COVERAGE_GAIN: f32 = 0.35;
/// How much of the layers' own colour the clouds take (the rest white).
pub const LAYER_TINT_SHARE: f32 = 0.6;
/// The drift with no wind (units a second over the ground), and units a second per unit of `windVelocity`.
pub const DEFAULT_DRIFT: [f32; 2] = [6.5, 3.5];
pub const WIND_UNITS: f32 = 6.0;

/// One server cloud layer: its texture's mean alpha, its alpha-weighted mean colour (gamma), its height share and speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudLayer {
    pub alpha: f32,
    pub color: [f32; 3],
    pub height: f32,
    pub speed: f32,
}

/// What the server's Sky gives the clouds.
#[derive(Debug, Clone, PartialEq)]
pub struct CloudSky {
    /// The server's cloud layers with a texture (none: the defaults).
    pub layers: Vec<CloudLayer>,
    /// The share of the sky they cover (0..1) and their density's gain.
    pub coverage: f32,
    pub density: f32,
    /// Their albedo (linear).
    pub tint: [f32; 3],
    /// Their drift over the ground (units a second).
    pub drift: [f32; 2],
    /// The server sky's mean linear colour above (from 17 degrees up) and near the horizon (2 to 9 degrees).
    pub ambient_top: [f32; 3],
    pub ambient_horizon: [f32; 3],
    /// How far every layer is raised over its [`FAIR_WEATHER`] height (units; 0 but over a map that rises into the deck).
    pub lift: f32,
}

impl CloudSky {
    /// The clouds' settings from the server's layers, wind and sky colours (`ambient_*` linear).
    pub fn from_layers(
        layers: Vec<CloudLayer>,
        wind: [f32; 3],
        ambient_top: [f32; 3],
        ambient_horizon: [f32; 3],
    ) -> CloudSky {
        let (coverage, density, tint) = if layers.is_empty() {
            (DEFAULT_COVERAGE, 1.0, [1.0; 3])
        } else {
            let clear: f32 = layers
                .iter()
                .map(|l| 1.0 - l.alpha.clamp(0.0, 1.0))
                .product();
            let combined = 1.0 - clear;
            let weight: f32 = layers.iter().map(|l| l.alpha.max(1e-3)).sum();
            let mean = [0, 1, 2].map(|k| {
                layers
                    .iter()
                    .map(|l| l.color[k] * l.alpha.max(1e-3))
                    .sum::<f32>()
                    / weight
            });
            let tint = mean.map(|c| {
                let own = (c / 0.9).clamp(0.0, 1.0);
                srgb_to_linear(1.0 + (own - 1.0) * LAYER_TINT_SHARE)
            });
            (
                (LAYER_COVERAGE_FLOOR + LAYER_COVERAGE_GAIN * combined).clamp(0.0, MAX_COVERAGE),
                1.0 + 0.5 * combined,
                tint,
            )
        };
        let w = [wind[0], wind[1]];
        let len = (w[0] * w[0] + w[1] * w[1]).sqrt();
        let drift = if len > 1e-4 && len.is_finite() {
            let speed = if layers.is_empty() {
                1.0
            } else {
                (layers.iter().map(|l| l.speed).sum::<f32>() / layers.len() as f32).max(1.0)
            };
            w.map(|v| v * WIND_UNITS * speed)
        } else {
            DEFAULT_DRIFT
        };
        CloudSky {
            layers,
            coverage,
            density,
            tint,
            drift,
            ambient_top,
            ambient_horizon,
            lift: 0.0,
        }
    }

    /// The clouds of `world`'s Sky (None without one), `sky` the scene's sky draws (the cloud layers' textures in `table`), the
    /// ambient from `ambient` (the server sky's colour toward a direction, gamma).
    pub fn of_world(
        src: &dyn SceneSource,
        sky: &[HdDraw],
        table: &TextureTable,
        ambient: &dyn Fn([f32; 3]) -> [f32; 3],
    ) -> Option<CloudSky> {
        let record = src.sky()?;
        let main = record.main.as_ref();
        let mut layers = Vec::new();
        for d in sky {
            let Some(n) = d.label.strip_prefix("sky cloud layer ") else {
                continue;
            };
            let Some(t) = d.texture.and_then(|(i, _)| table.textures.get(i)) else {
                continue;
            };
            let i: usize = n.trim().parse().unwrap_or(0);
            let (alpha, color) = texture_cover(t);
            let (height, speed) = main
                .and_then(|m| m.clouds.get(i))
                .map_or((0.0, 0.0), |t| (t.height, t.speed));
            layers.push(CloudLayer {
                alpha,
                color,
                height,
                speed,
            });
        }
        let wind = record.wind.unwrap_or([0.0; 3]);
        let (top, horizon) = sky_ambient(ambient);
        Some(CloudSky::from_layers(layers, wind, top, horizon))
    }
}

/// A texture's mean alpha and its alpha-weighted mean colour (gamma 0..1), from the level no wider than 64 texels.
pub fn texture_cover(t: &crate::Texture) -> (f32, [f32; 3]) {
    let mut level = 0;
    let mut w = t.width.max(1);
    while w > 64 && level + 1 < t.levels.len() {
        w = (w / 2).max(1);
        level += 1;
    }
    let px = &t.levels[level];
    let n = px.len() / 4;
    if n == 0 {
        return (0.0, [1.0; 3]);
    }
    let (mut a, mut c) = (0.0f64, [0.0f64; 3]);
    for p in px.chunks_exact(4) {
        let al = f64::from(p[3]) / 255.0;
        a += al;
        for k in 0..3 {
            c[k] += f64::from(p[k]) / 255.0 * al;
        }
    }
    let colour = if a > 1e-6 {
        c.map(|v| (v / a) as f32)
    } else {
        [1.0; 3]
    };
    ((a / n as f64) as f32, colour)
}

/// The server sky's mean linear colour above (elevations from 17 degrees up) and near the horizon (2 to 9 degrees), from
/// `radiance` (gamma, toward a unit direction): 96 directions each, a Fibonacci spiral over each band.
pub fn sky_ambient(radiance: &dyn Fn([f32; 3]) -> [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let band = |z0: f32, z1: f32| {
        let n = 96;
        let mut sum = [0.0f32; 3];
        for i in 0..n {
            let z = z0 + (z1 - z0) * (i as f32 + 0.5) / n as f32;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let phi = i as f32 * 2.399_963_1;
            let c = radiance([r * phi.cos(), r * phi.sin(), z]);
            for k in 0..3 {
                sum[k] += srgb_to_linear(c[k].clamp(0.0, 1.0));
            }
        }
        sum.map(|v| v / n as f32)
    };
    (
        band(17f32.to_radians().sin(), 1.0),
        band(2f32.to_radians().sin(), 9f32.to_radians().sin()),
    )
}

/// The clouds' displacement over the ground at `time` seconds (they drift with `drift`), wrapped to keep the numbers small (the
/// weather map's period over the turned domain: a multiple of every volume's period is not needed, the wrap only steps once in many
/// hours at a breeze).
pub fn drift_at(drift: [f32; 2], time: f32) -> [f32; 2] {
    let wrap = WEATHER_PERIOD * 64.0;
    drift.map(|v| (v * time).rem_euclid(wrap))
}

/// The shadow map's centre and span around `eye` for a sun toward `sun`: the eye's projection onto the base's plane along the
/// sun, snapped to the map's texels (the shadows stay still under a moving camera), and twice the visible distance plus a margin.
pub fn shadow_window(eye: [f32; 3], sun: [f32; 3], visible: f32) -> ([f32; 2], f32) {
    shadow_window_at(eye, sun, visible, SLAB.base)
}

/// [`shadow_window`] on the plane at height `base` (the lowest layer drawn).
pub fn shadow_window_at(eye: [f32; 3], sun: [f32; 3], visible: f32, base: f32) -> ([f32; 2], f32) {
    let span = (2.0 * visible + 200.0).max(SHADOW_MIN_SPAN);
    let texel = span / SHADOW_TEXELS as f32;
    let len = (sun[0] * sun[0] + sun[1] * sun[1] + sun[2] * sun[2])
        .sqrt()
        .max(1e-6);
    let s = sun.map(|v| v / len);
    let up = s[2].max(0.05);
    let rise = base - eye[2];
    let c = [eye[0] + s[0] * rise / up, eye[1] + s[1] * rise / up];
    (c.map(|v| (v / texel).round() * texel), span)
}

/// The noise volumes (RGBA8 unless named): the shape (r Perlin-Worley, gba Worley fBm), the detail (rgb Worley fBm), the weather
/// map (r coverage variation, g the clouds' kind, b where the altocumulus lies in rows, a the rows' slow bend), the blue noise (one
/// byte a texel: its rank over 256), the cirrus plane (r its fibres, g its patches, b a second set of fibres).
pub struct CloudNoise {
    pub shape: Vec<u8>,
    pub detail: Vec<u8>,
    pub weather: Vec<u8>,
    pub blue: Vec<u8>,
    pub cirrus: Vec<u8>,
}

static NOISE: OnceLock<CloudNoise> = OnceLock::new();

/// The noise, generated once per process (a fraction of a second on the worker threads) and shared.
pub fn noise() -> &'static CloudNoise {
    NOISE.get_or_init(generate)
}

/// Whether the noise is made already.
pub fn noise_ready() -> bool {
    NOISE.get().is_some()
}

/// The noise volumes, generated afresh (deterministic: the same bytes every run).
pub fn generate() -> CloudNoise {
    let shape = ShapeTables::new();
    let detail = [
        CellTable::new(2, SEED ^ 0x55),
        CellTable::new(3, SEED ^ 0x66),
        CellTable::new(4, SEED ^ 0x77),
        CellTable::new(8, SEED ^ 0x78),
        CellTable::new(16, SEED ^ 0x79),
    ];
    let weather = [
        GradientTable::new(5, SEED ^ 0x88),
        GradientTable::new(10, SEED ^ 0x89),
        GradientTable::new(20, SEED ^ 0x8a),
        GradientTable::new(40, SEED ^ 0x8b),
        GradientTable::new(3, SEED ^ 0x99),
        GradientTable::new(6, SEED ^ 0x9a),
        GradientTable::new(4, SEED ^ 0xa1),
        GradientTable::new(8, SEED ^ 0xa2),
        GradientTable::new(16, SEED ^ 0xa3),
        GradientTable::new(3, SEED ^ 0xb1),
        GradientTable::new(6, SEED ^ 0xb2),
    ];
    let mut weather_map = volume(WEATHER_SIZE, 1, &|p| {
        let q = [p[0], p[1], 0.37];
        let r = gradient_fbm(q, &weather[..4]);
        let r = (0.5 + (r - 0.5) * 1.6).clamp(0.0, 1.0);
        [
            r,
            gradient_fbm(q, &weather[4..6]),
            gradient_fbm(q, &weather[6..9]),
            gradient_fbm(q, &weather[9..]),
        ]
    });
    // The altocumulus' patches by their rank (an even spread: the shader's thresholds are shares of the sky).
    equalize(&mut weather_map, 2);
    CloudNoise {
        shape: volume(SHAPE_SIZE, SHAPE_SIZE, &|p| shape.texel(p)),
        detail: volume(DETAIL_SIZE, DETAIL_SIZE, &|p| {
            [
                fbm3(p, &detail[0], &detail[1], &detail[2]),
                fbm3(p, &detail[1], &detail[2], &detail[3]),
                fbm3(p, &detail[2], &detail[3], &detail[4]),
                1.0,
            ]
        }),
        weather: weather_map,
        blue: blue_noise(BLUE_SIZE)
            .into_iter()
            .map(|r| (r * 256 / (BLUE_SIZE * BLUE_SIZE)) as u8)
            .collect(),
        cirrus: cirrus_plane(CIRRUS_SIZE),
    }
}

/// A tiling 2D lattice of `nx` x `ny` cells with a hashed unit gradient at each point (cells longer along x than y draw streaks
/// along x).
pub struct Gradient2 {
    nx: i32,
    ny: i32,
    g: Vec<[f32; 2]>,
}

impl Gradient2 {
    pub fn new(nx: i32, ny: i32, seed: u32) -> Gradient2 {
        let (nx, ny) = (nx.max(1), ny.max(1));
        let mut g = Vec::with_capacity((nx * ny) as usize);
        for y in 0..ny {
            for x in 0..nx {
                let a = unit(hash(x, y, 11, seed)) * std::f32::consts::TAU;
                g.push([a.cos(), a.sin()]);
            }
        }
        Gradient2 { nx, ny, g }
    }

    /// Gradient noise at `p` (unit coordinates of the tile), about [-1, 1], with the quintic fade.
    pub fn at(&self, p: [f32; 2]) -> f32 {
        let q = [p[0] * self.nx as f32, p[1] * self.ny as f32];
        let i = [q[0].floor(), q[1].floor()];
        let f = [q[0] - i[0], q[1] - i[1]];
        let i = [i[0] as i32, i[1] as i32];
        let corner = |ox: i32, oy: i32| -> f32 {
            let cx = (i[0] + ox).rem_euclid(self.nx);
            let cy = (i[1] + oy).rem_euclid(self.ny);
            let g = self.g[(cy * self.nx + cx) as usize];
            g[0] * (f[0] - ox as f32) + g[1] * (f[1] - oy as f32)
        };
        let u = [fade(f[0]), fade(f[1])];
        let a = lerp(corner(0, 0), corner(1, 0), u[0]);
        let b = lerp(corner(0, 1), corner(1, 1), u[0]);
        // Unit gradients leave 2D gradient noise within about ±0.71; scaled toward ±1.
        lerp(a, b, u[1]) * 1.41
    }
}

/// `v`'s values stretched so that its 3rd and 97th percentiles land on 0.04 and 0.96 (each channel of the cirrus noise spans the
/// same range whatever its octaves' sum).
fn stretch(v: &mut [f32]) {
    let mut sorted = v.to_vec();
    sorted.sort_by(f32::total_cmp);
    let n = sorted.len().max(1);
    let lo = sorted[(n * 3 / 100).min(n - 1)];
    let hi = sorted[(n * 97 / 100).min(n - 1)];
    for x in v.iter_mut() {
        *x = (0.04 + 0.92 * (*x - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0);
    }
}

/// The cirrus sheet's noise, `size` x `size` RGBA8 (tiles): r its fibres (ridged gradient octaves on cells five to six times longer
/// along x than across, over a soft streaked base, the domain bent by a slow warp: hooked, combed strands), g its patches (round
/// gradient octaves), b a second, coarser set of fibres. Every octave at least four texels a cell across.
pub fn cirrus_plane(size: usize) -> Vec<u8> {
    let s = SEED ^ 0xc1c1;
    let warp = [
        Gradient2::new(3, 3, s ^ 1),
        Gradient2::new(3, 3, s ^ 2),
        Gradient2::new(7, 7, s ^ 3),
        Gradient2::new(7, 7, s ^ 4),
    ];
    let fibres = [
        Gradient2::new(2, 12, s ^ 10),
        Gradient2::new(4, 24, s ^ 11),
        Gradient2::new(8, 48, s ^ 12),
        Gradient2::new(16, 96, s ^ 13),
    ];
    let soft = [
        Gradient2::new(2, 6, s ^ 20),
        Gradient2::new(4, 12, s ^ 21),
        Gradient2::new(8, 24, s ^ 22),
    ];
    let patches = [
        Gradient2::new(2, 2, s ^ 30),
        Gradient2::new(4, 4, s ^ 31),
        Gradient2::new(8, 8, s ^ 32),
    ];
    let second = [
        Gradient2::new(3, 12, s ^ 40),
        Gradient2::new(6, 24, s ^ 41),
        Gradient2::new(12, 48, s ^ 42),
    ];
    let texel = |p: [f32; 2]| -> [f32; 3] {
        let q = [
            p[0] + 0.07 * warp[0].at(p) + 0.02 * warp[2].at(p),
            p[1] + 0.07 * warp[1].at(p) + 0.02 * warp[3].at(p),
        ];
        let ridged = |octaves: &[Gradient2], weights: &[f32]| -> f32 {
            let (mut sum, mut norm) = (0.0, 0.0);
            for (o, w) in octaves.iter().zip(weights) {
                let r = 1.0 - o.at(q).abs().min(1.0);
                sum += w * r * r * r;
                norm += w;
            }
            sum / norm
        };
        let smooth = |octaves: &[Gradient2], at: [f32; 2]| -> f32 {
            let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
            for o in octaves {
                sum += amp * o.at(at);
                norm += amp;
                amp *= 0.5;
            }
            0.5 + 0.5 * sum / norm
        };
        let fib = ridged(&fibres, &[1.0, 0.6, 0.4, 0.25]);
        [
            0.55 * fib + 0.45 * smooth(&soft, q),
            smooth(&patches, p),
            ridged(&second, &[1.0, 0.55, 0.3]),
        ]
    };
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 32);
    let rows: Vec<Vec<[f32; 3]>> = std::thread::scope(|sc| {
        let jobs: Vec<_> = (0..workers)
            .map(|w| {
                let texel = &texel;
                sc.spawn(move || {
                    (w..size)
                        .step_by(workers)
                        .map(|y| {
                            let row: Vec<[f32; 3]> = (0..size)
                                .map(|x| {
                                    texel([
                                        (x as f32 + 0.5) / size as f32,
                                        (y as f32 + 0.5) / size as f32,
                                    ])
                                })
                                .collect();
                            (y, row)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut all: Vec<(usize, Vec<[f32; 3]>)> = jobs
            .into_iter()
            .flat_map(|j| j.join().unwrap_or_default())
            .collect();
        all.sort_by_key(|(y, _)| *y);
        all.into_iter().map(|(_, r)| r).collect()
    });
    let flat: Vec<[f32; 3]> = rows.concat();
    let mut channels: Vec<Vec<f32>> = (0..3)
        .map(|k| flat.iter().map(|t| t[k]).collect())
        .collect();
    for c in &mut channels {
        stretch(c);
    }
    let mut out = Vec::with_capacity(size * size * 4);
    for i in 0..size * size {
        for c in &channels {
            out.push((c[i] * 255.0).round() as u8);
        }
        out.push(255);
    }
    out
}

/// An RGBA8 volume `size` x `size` x `depth`, each texel from `f` at the unit coordinates of its centre, z slices shared out over
/// the worker threads.
fn volume(size: usize, depth: usize, f: &(dyn Fn([f32; 3]) -> [f32; 4] + Sync)) -> Vec<u8> {
    let slice = |z: usize| -> Vec<u8> {
        let mut out = Vec::with_capacity(size * size * 4);
        for y in 0..size {
            for x in 0..size {
                let p = [
                    (x as f32 + 0.5) / size as f32,
                    (y as f32 + 0.5) / size as f32,
                    (z as f32 + 0.5) / depth.max(1) as f32,
                ];
                for v in f(p) {
                    out.push((v.clamp(0.0, 1.0) * 255.0).round() as u8);
                }
            }
        }
        out
    };
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 32);
    let slices: Vec<Vec<u8>> = std::thread::scope(|s| {
        let jobs: Vec<_> = (0..workers)
            .map(|w| {
                let slice = &slice;
                s.spawn(move || {
                    (w..depth)
                        .step_by(workers)
                        .map(|z| (z, slice(z)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut all: Vec<(usize, Vec<u8>)> = jobs
            .into_iter()
            .flat_map(|j| j.join().unwrap_or_default())
            .collect();
        all.sort_by_key(|(z, _)| *z);
        all.into_iter().map(|(_, v)| v).collect()
    });
    slices.concat()
}

/// Our lattice hash: the point and the seed mixed, then a 32-bit avalanche (the multipliers are odd constants with good bit spread).
pub fn hash(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = seed.wrapping_mul(0x9e37_79b1);
    for v in [x, y, z] {
        h ^= (v as u32).wrapping_mul(0x85eb_ca77);
        h = h.rotate_left(15).wrapping_mul(0xc2b2_ae3d);
    }
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

/// A hash as a number in [0, 1).
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / 16_777_216.0
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// A tiling lattice of `n` cells a side with a hashed unit gradient at each point (a point on the sphere from two hashes).
pub struct GradientTable {
    n: i32,
    g: Vec<[f32; 3]>,
}

impl GradientTable {
    pub fn new(n: i32, seed: u32) -> GradientTable {
        let n = n.max(1);
        let mut g = Vec::with_capacity((n * n * n) as usize);
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let zz = 1.0 - 2.0 * unit(hash(x, y, z, seed));
                    let a = unit(hash(x, y, z, seed ^ 0x5bd1_e995)) * std::f32::consts::TAU;
                    let r = (1.0 - zz * zz).max(0.0).sqrt();
                    g.push([r * a.cos(), r * a.sin(), zz]);
                }
            }
        }
        GradientTable { n, g }
    }

    /// Gradient noise at `p` (unit coordinates of the tile), about [-1, 1], with the quintic fade.
    pub fn at(&self, p: [f32; 3]) -> f32 {
        let n = self.n;
        let q = p.map(|v| v * n as f32);
        let i = q.map(|v| v.floor());
        let f = [q[0] - i[0], q[1] - i[1], q[2] - i[2]];
        let i = i.map(|v| v as i32);
        let corner = |o: [i32; 3]| -> f32 {
            let c = [0, 1, 2].map(|k| (i[k] + o[k]).rem_euclid(n));
            let g = self.g[((c[2] * n + c[1]) * n + c[0]) as usize];
            let d = [0, 1, 2].map(|k| f[k] - o[k] as f32);
            g[0] * d[0] + g[1] * d[1] + g[2] * d[2]
        };
        let u = f.map(fade);
        let x00 = lerp(corner([0, 0, 0]), corner([1, 0, 0]), u[0]);
        let x10 = lerp(corner([0, 1, 0]), corner([1, 1, 0]), u[0]);
        let x01 = lerp(corner([0, 0, 1]), corner([1, 0, 1]), u[0]);
        let x11 = lerp(corner([0, 1, 1]), corner([1, 1, 1]), u[0]);
        // Unit gradients leave 3D gradient noise within about ±0.87; scaled toward ±1.
        lerp(lerp(x00, x10, u[1]), lerp(x01, x11, u[1]), u[2]) * 1.15
    }
}

/// A tiling lattice of `n` cells a side with a hashed feature point in each cell.
pub struct CellTable {
    n: i32,
    pts: Vec<[f32; 3]>,
}

impl CellTable {
    pub fn new(n: i32, seed: u32) -> CellTable {
        let n = n.max(1);
        let mut pts = Vec::with_capacity((n * n * n) as usize);
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    pts.push([
                        unit(hash(x, y, z, seed)),
                        unit(hash(x, y, z, seed ^ 0x68e3_1da4)),
                        unit(hash(x, y, z, seed ^ 0x1b56_c4e9)),
                    ]);
                }
            }
        }
        CellTable { n, pts }
    }

    /// Cell noise at `p` (unit coordinates of the tile): 1 at a feature point, falling with the distance to the nearest (in cells)
    /// to 0 a cell away.
    pub fn at(&self, p: [f32; 3]) -> f32 {
        let n = self.n;
        let q = p.map(|v| v * n as f32);
        let i = q.map(|v| v.floor() as i32);
        let mut best = f32::MAX;
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let c = [i[0] + dx, i[1] + dy, i[2] + dz];
                    let w = c.map(|v| v.rem_euclid(n));
                    let h = self.pts[((w[2] * n + w[1]) * n + w[0]) as usize];
                    let d = [0, 1, 2].map(|k| c[k] as f32 + h[k] - q[k]);
                    best = best.min(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
                }
            }
        }
        (1.0 - best.sqrt()).clamp(0.0, 1.0)
    }
}

/// Three cell octaves (weights 0.625, 0.25, 0.125).
fn fbm3(p: [f32; 3], a: &CellTable, b: &CellTable, c: &CellTable) -> f32 {
    0.625 * a.at(p) + 0.25 * b.at(p) + 0.125 * c.at(p)
}

/// Gradient octaves (each half the weight of the last) in [0, 1].
fn gradient_fbm(p: [f32; 3], octaves: &[GradientTable]) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    for o in octaves {
        sum += amp * o.at(p);
        norm += amp;
        amp *= 0.5;
    }
    (0.5 + 0.5 * sum / norm.max(1e-6)).clamp(0.0, 1.0)
}

/// `v` from `[lo, hi]` onto `[0, 1]` (unclamped).
pub fn remap(v: f32, lo: f32, hi: f32) -> f32 {
    (v - lo) / (hi - lo).max(1e-6)
}

/// The shape volume's lattices: the Perlin-Worley base's gradient octaves (4, 8, 16 cells) and cell octaves (5, 10, 20), and the
/// erosion's cell octaves (4 to 32: at least four texels a cell over the volume's 128).
struct ShapeTables {
    perlin: [GradientTable; 3],
    worley: [CellTable; 3],
    erosion: [CellTable; 5],
}

impl ShapeTables {
    fn new() -> ShapeTables {
        ShapeTables {
            perlin: [
                GradientTable::new(4, SEED),
                GradientTable::new(8, SEED ^ 0x9e37),
                GradientTable::new(16, SEED ^ 0x13c6e),
            ],
            worley: [
                CellTable::new(5, SEED ^ 0x11),
                CellTable::new(10, SEED ^ 0x12),
                CellTable::new(20, SEED ^ 0x13),
            ],
            erosion: [
                CellTable::new(4, SEED ^ 0x22),
                CellTable::new(8, SEED ^ 0x33),
                CellTable::new(12, SEED ^ 0x44),
                CellTable::new(24, SEED ^ 0x45),
                CellTable::new(32, SEED ^ 0x46),
            ],
        }
    }

    /// r the Perlin-Worley base (the gradient fBm lifted where the cells are: billows with gaps between them), gba cell fBm from
    /// 4, 8 and 12 cells (the shape's erosion at rising frequencies).
    fn texel(&self, p: [f32; 3]) -> [f32; 4] {
        let perlin = gradient_fbm(p, &self.perlin);
        let worley = fbm3(p, &self.worley[0], &self.worley[1], &self.worley[2]);
        let pw = remap(perlin, worley - 1.0, 1.0).clamp(0.0, 1.0);
        let e = &self.erosion;
        [
            pw,
            fbm3(p, &e[0], &e[1], &e[2]),
            fbm3(p, &e[1], &e[2], &e[3]),
            fbm3(p, &e[2], &e[3], &e[4]),
        ]
    }
}

/// Channel `c` of an RGBA8 buffer spread evenly over the bytes by its own histogram (each byte its value's rank).
fn equalize(data: &mut [u8], c: usize) {
    let mut count = [0usize; 256];
    for t in data.chunks_exact(4) {
        count[usize::from(t[c])] += 1;
    }
    let total = (data.len() / 4).max(1);
    let mut below = 0;
    let mut rank = [0u8; 256];
    for (v, n) in count.iter().enumerate() {
        // The middle of the value's span of ranks.
        rank[v] = (((below as f32 + *n as f32 * 0.5) / total as f32) * 255.0).round() as u8;
        below += n;
    }
    for t in data.chunks_exact_mut(4) {
        t[c] = rank[usize::from(t[c])];
    }
}

/// The mip chain of an RGBA8 volume `size` x `size` x `depth` (`depth` 1: a plane), level 0 first: each level the 2 x 2 x 2 (or
/// 2 x 2) box average of the one above, down to one texel a side. Each entry: (size, depth, bytes).
pub fn mips(data: &[u8], size: usize, depth: usize) -> Vec<(usize, usize, Vec<u8>)> {
    let mut out = vec![(size, depth, data.to_vec())];
    while out.last().is_some_and(|(s, _, _)| *s > 1) {
        let (s, d, src) = out.last().cloned().unwrap_or_default();
        let (n, nd) = ((s / 2).max(1), (d / 2).max(1));
        let mut dst = vec![0u8; n * n * nd * 4];
        let dz = if d > 1 { 2 } else { 1 };
        for z in 0..nd {
            for y in 0..n {
                for x in 0..n {
                    for c in 0..4 {
                        let mut sum = 0u32;
                        for k in 0..dz {
                            for j in 0..2 {
                                for i in 0..2 {
                                    let at =
                                        (((z * dz + k) * s + y * 2 + j) * s + x * 2 + i) * 4 + c;
                                    sum += u32::from(src[at]);
                                }
                            }
                        }
                        let count = 4 * dz as u32;
                        dst[((z * n + y) * n + x) * 4 + c] = ((sum + count / 2) / count) as u8;
                    }
                }
            }
        }
        out.push((n, nd, dst));
    }
    out
}

/// A blue-noise rank map `n` x `n` (each rank 0..n² once) by Ulichney's void-and-cluster: a Gaussian energy (sigma 1.5, on the
/// torus) over the set points; the initial pattern's clusters moved into voids until stable, then ranked by removing the tightest
/// cluster (down from the pattern's count) and filling the largest void (up to the end).
pub fn blue_noise(n: usize) -> Vec<usize> {
    let total = n * n;
    let sigma2 = 2.0 * 1.5f32 * 1.5;
    // The kernel by toroidal offset.
    let kernel: Vec<f32> = (0..total)
        .map(|i| {
            let (x, y) = (i % n, i / n);
            let dx = x.min(n - x) as f32;
            let dy = y.min(n - y) as f32;
            (-(dx * dx + dy * dy) / sigma2).exp()
        })
        .collect();
    let add = |energy: &mut [f32], at: usize, sign: f32| {
        let (ax, ay) = (at % n, at / n);
        for y in 0..n {
            let ky = (y + n - ay) % n;
            for x in 0..n {
                let kx = (x + n - ax) % n;
                energy[y * n + x] += sign * kernel[ky * n + kx];
            }
        }
    };
    let pick = |energy: &[f32], set: &[bool], want: bool, max: bool| -> usize {
        let mut best = None;
        for i in 0..total {
            if set[i] != want {
                continue;
            }
            let better = match best {
                None => true,
                Some((_, e)) => {
                    if max {
                        energy[i] > e
                    } else {
                        energy[i] < e
                    }
                }
            };
            if better {
                best = Some((i, energy[i]));
            }
        }
        best.map_or(0, |(i, _)| i)
    };
    // The initial pattern: a tenth of the points, hashed.
    let ones = (total / 10).max(1);
    let mut set = vec![false; total];
    let mut energy = vec![0.0f32; total];
    let mut placed = 0;
    let mut k = 0u32;
    while placed < ones {
        let i = (hash(k as i32, 7, 3, SEED) as usize) % total;
        k += 1;
        if !set[i] {
            set[i] = true;
            add(&mut energy, i, 1.0);
            placed += 1;
        }
    }
    // Clusters into voids until the tightest cluster is the largest void.
    for _ in 0..total {
        let c = pick(&energy, &set, true, true);
        set[c] = false;
        add(&mut energy, c, -1.0);
        let v = pick(&energy, &set, false, false);
        set[v] = true;
        add(&mut energy, v, 1.0);
        if v == c {
            break;
        }
    }
    let mut rank = vec![0usize; total];
    // Phase 1: the pattern's points ranked down by removing the tightest cluster.
    {
        let mut s = set.clone();
        let mut e = energy.clone();
        for r in (0..ones).rev() {
            let c = pick(&e, &s, true, true);
            s[c] = false;
            add(&mut e, c, -1.0);
            rank[c] = r;
        }
    }
    // Phase 2 and 3: the rest ranked up by filling the largest void.
    for r in ones..total {
        let v = pick(&energy, &set, false, false);
        set[v] = true;
        add(&mut energy, v, 1.0);
        rank[v] = r;
    }
    rank
}

#[cfg(test)]
#[path = "clouds_tests.rs"]
mod tests;
