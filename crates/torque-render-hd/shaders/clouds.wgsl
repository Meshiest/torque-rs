// The volumetric clouds (the data and the noise are hd::clouds): a small list of layers from the lowest up, each with
// its own height, thickness, coverage, wind and light, marched per texel at a reduced size, lit by the sun through the atmosphere
// at the layer's own height and by the sky, kept in a history of their own.
//
// - The layers: a volume (fair-weather cumulus, or a thin sheet of small rounded altocumulus elements) is marched through its slab;
//   a sheet (cirrus) is looked up once where the ray crosses its height, in its own fibred noise. They are composited front to back
//   (up through them from the lowest, down from the highest; the eye in a layer, over it or under it), and each layer's sun is shaded by the layers above it (their columns where the sun's ray
//   crosses them) and its undersides lit by the sunlit tops of those below.
// - fs_sun_rays and fs_sun_mean: the sun's transmittance through the layers as the eye sees it (rays across the sun's disc), for
//   a sun too low for the shadow map (see their own comment at the end).
// - fs_shadow (the cloud shadow map, SHADOW_TEXELS a side around the eye on the lowest base's plane): each texel's sun ray up through
//   the lowest slab (the shape without its detail) and the columns of the layers above, the transmittance exp(-od). The forward
//   pass (the clouds' shadows on the ground at Ultra) and the volumetric light (no god rays under a cloud) read it through
//   cloudshadow.wgsl.
// - fs_march: the view ray through the texel's centre (the unjittered camera, from the eye wherever it is: a segment that starts
//   at the eye takes its steps closer near it), through each layer no farther than its reach, a volume's segment held to
//   SEGMENT_MAX so the steps stay short, in the layer's steps, the start offset by the blue noise turned by the golden ratio each
//   frame (the history averages it). A volume's
//   density: the weather map's coverage (puffs with clear sky between), its height profile, the Perlin-Worley shape eroded by its
//   Worley fBm, a contrast curve that sharpens the edges more toward the tops, then the detail volume's erosion. The light: Beer's
//   law toward the sun in multiple-scattering octaves (Wrenninge 2013: each octave's energy, extinction and eccentricity scaled
//   down) of a dual-lobe Henyey-Greenstein phase, a powder term away from the sun, the sun's light through the atmosphere from the
//   layer's height (the transmittance table toward it, over its value overhead: a low sun lights them peach and pink, and a high
//   layer keeps the sun after it has set for the low deck); the ambient from the sky above at the tops and the horizon and the
//   ground below (the atmosphere's own sky: its light; the server's box: its mean colours), dimmed in the cloud's lower parts.
//   Each step integrates as Hillaire 2016 writes it (an albedo of 1: S (1 - exp(-sigma dt))). Aerial perspective: with our own sky
//   the air between the eye and the cloud takes it toward the sky behind it (the cloud still hides what is behind it); with the
//   server's box far clouds fade out instead. Out: the light toward the eye (rgb) and the transmittance (a); the seen-weighted mean
//   distance and the layers' motion since the last frame.
// - fs_resolve: the history, reprojected (that distance's point moved back by that motion, through last frame's camera) and read
//   with a Catmull-Rom filter, held within the current neighbourhood's mean and spread (3 x 3: its colour and its transmittance)
//   and mixed with the current texel by the quality's weight; a texel whose point was off screen starts afresh.
//
// Prepended: common.wgsl, skycommon.wgsl.

struct Layer {
    // x: the base, y: the top (world units; a sheet: its height twice), z: the form (0 a volume, 1 a sheet), w: the march's steps.
    slab: vec4<f32>,
    // x: the coverage, y: the extinction per unit at density 1 (a sheet: its optical depth straight up), z: 1 / the shape's period
    // (a sheet: the fibres'), w: 1 / the detail's.
    shape: vec4<f32>,
    // x: 1 / the weather map's period (a sheet: its patches'), y: the detail's erosion, z: the coverage's variation, w: the light
    // march's steps.
    domain: vec4<f32>,
    // xy: the noise domain's offset (the drift), z: the slow change of shape, w: the shape's texel (units).
    offset: vec4<f32>,
    // xy: how far the layer moved since the last frame (world units), zw: the domain's turn (cos, sin).
    motion: vec4<f32>,
    // x: the edges' cut at the base, y toward the top, z: the tops' extra density, w: the profile (0 cumulus heaps, 1 elements).
    form: vec4<f32>,
    // x: the aerial perspective's distance, y z: the far fade's start and end, w: the reach (units along the ray).
    far: vec4<f32>,
    // x: the sun's gain, y: the ambient's, z: the atmosphere's radius at the layer's middle (km), w: the share of the sky the layers
    // under it cover (their sunlit tops light its undersides).
    light: vec4<f32>,
};

struct Clouds {
    layers: array<Layer, 3>,
    // x: the layers, y: the march's start height, z: the server box's far fade (units), w: 1 when the history is valid.
    count: vec4<f32>,
    // xyz: toward the sun; w: the powder's strength.
    sun: vec4<f32>,
    // rgb: the sun's linear light; w: the multiple-scattering octaves.
    sun_light: vec4<f32>,
    // rgb: the server sky's linear colour above; w: the ambient's floor in the clouds' lower parts.
    amb_top: vec4<f32>,
    // rgb: the server sky's undersides' light (its horizon's and the ground's); w: the diffuse multiple scattering's share.
    amb_bottom: vec4<f32>,
    // rgb: the clouds' albedo; w: how much slower the diffuse multiple scattering decays.
    tint: vec4<f32>,
    // x: the forward lobe's asymmetry, y: the back lobe's, z: the back lobe's share, w: the light march's first step (units).
    phase: vec4<f32>,
    // x: the frame's index, y: the history's weight, z: internal pixels per texel, w: 1 with the detail volume.
    march: vec4<f32>,
    // xy: the internal frame's share of the texels' span (the uv scale), zw: the texels across and down.
    uvmap: vec4<f32>,
    // xy: the shadow map's centre, z: its span (units), w: its steps.
    map: vec4<f32>,
    // x: the pixel's angle (radians), y: the shadow map's texels a side, z: the history's spread (the box's half width in standard
    // deviations), w: 1 with the atmosphere's own sky (its light for the ambient and the air).
    lod: vec4<f32>,
    // The multiple-scattering octaves: x each one's energy, y its extinction, z its eccentricity (each a share of the last's); w how
    // much of the atmosphere's colour the sun's light takes.
    octaves: vec4<f32>,
    // x: the sunlit ground's share of the sun under the clouds, y: the horizon's share in the undersides' light, z: the share of a
    // higher layer's optical depth that shades a lower one, w: the most the sun's slant lengthens that path.
    under: vec4<f32>,
    // The light march's far shadow: x the share of the optical depth past the near steps that a sun on the horizon keeps (all of it
    // overhead), y the near steps, z w: the cosines to the sun over which a view toward it takes the easing back (looking at the light
    // through a cloud, its whole path shades it).
    reach: vec4<f32>,
    // x: the slant (the ray's path through a slab over its thickness) past which it thins out, y: its coverage's power (the spread),
    // z: its density's; w: the radius of the disc the rays toward the sun cover (fs_sun_rays; radians).
    thin: vec4<f32>,
    // x: how overcast the sky is (0 fair weather, 1 closed: the ambient loses its colour; hd::clouds's OVERCAST); y: 1 when sun_light
    // is the look's own sunlight (the air's transmittance dims it as well as coloring it); z: the refined march's fine step (units; 0
    // the even march), w: its most samples as a multiple of the layer's steps.
    weather: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<uniform> clouds: Clouds;
@group(1) @binding(1) var shape_tex: texture_3d<f32>;
@group(1) @binding(2) var detail_tex: texture_3d<f32>;
@group(1) @binding(3) var weather_tex: texture_2d<f32>;
@group(1) @binding(4) var blue_tex: texture_2d<f32>;
@group(1) @binding(5) var repeat_samp: sampler;
@group(1) @binding(6) var cirrus_tex: texture_2d<f32>;
// The resolve's group (its own layout; bindings apart from the march's).
@group(1) @binding(10) var<uniform> resolve_clouds: Clouds;
@group(1) @binding(11) var current_tex: texture_2d<f32>;
@group(1) @binding(12) var dist_tex: texture_2d<f32>;
@group(1) @binding(13) var history_tex: texture_2d<f32>;
@group(1) @binding(14) var clamp_samp: sampler;
// The march reads the sky's group: the atmosphere's tables.
@group(2) @binding(1) var<uniform> sky: Sky;
@group(2) @binding(2) var sky_view_lut: texture_2d<f32>;
@group(2) @binding(3) var transmittance_lut: texture_2d<f32>;
@group(2) @binding(4) var sky_linear: sampler;

const GOLDEN: f32 = 0.61803398875;
// The powder term's depth (units): how deep into the cloud the sun's light has scattered enough to be seen.
const POWDER_DEPTH: f32 = 50.0;
// The longest stretch of slab one ray marches (units): past it a low ray's steps would grow longer than the clouds' features.
const SEGMENT_MAX: f32 = 9000.0;
// How deep the sky's light is shaded inside the cloud (units at its extinction).
const AMBIENT_DEPTH: f32 = 18.0;
// The distance target holds the distance over this (half floats reach 65504).
const DIST_SCALE: f32 = 16.0;
// The sun's light fades in over this much of its cosine above a layer's horizon (the earth's shadow's soft edge).
const HORIZON_FADE: f32 = 0.012;
// The sheet's ice: its forward lobe's asymmetry and share, and its back lobe's asymmetry.
const ICE_FORWARD: f32 = 0.7;
const ICE_FORWARD_SHARE: f32 = 0.6;
const ICE_BACK: f32 = -0.2;
// The sheet fades in over these sines of the ray's elevation (a flat sheet at the horizon would show its edge).
const SHEET_LOW: f32 = 0.012;
const SHEET_HIGH: f32 = 0.07;
// The least sine of the ray's elevation a sheet thickens to (seen aslant it is thicker, but no more than this: the far sheet would
// otherwise close into a band along the horizon).
const SHEET_SLANT: f32 = 0.25;
// The cirrus' veil under an overcast (clouds.weather.x): from x to y of its coverage its fibres give way (at most z of them) to an
// even sheet of w times its coverage.
const CIRRUS_VEIL: vec4<f32> = vec4<f32>(0.55, 0.9, 0.65, 0.5);
// How much of the history's weight a texel that moved 8 or more texels since the last frame gives up.
const SPIN_EASE: f32 = 0.35;
// The view's transmittance under which a sample's light march takes one step fewer, and under which two.
const DEEP_SEEN: vec2<f32> = vec2<f32>(0.35, 0.12);
// The refined march's fine steps in a row that find no cloud before it takes long steps again.
const FINE_EMPTY: u32 = 3u;
// The altocumulus' rows: how many across a shape period, and how much of an element's density they set where it lies in rows.
const ELEMENT_ROWS: f32 = 3.0;
const ELEMENT_ROW_SHARE: f32 = 0.7;
// The breaks along the rows: the coarse cells' noise (s.g) under x leaves a gap, over y the row runs on (runs of a few cells to
// many, out of step with the next row's).
const ELEMENT_BREAKS: vec2<f32> = vec2<f32>(0.42, 0.5);
// How many rows the weather map's slow bend shifts them (rows bent and turned over kilometres, not ruled).
const ELEMENT_BEND: f32 = 3.0;
// The weather map's b: rows fade in from x to y and out from z to w; loose elements in no rows past them, the layer clear under
// about x - 0.08 (element_present).
const ELEMENT_PATCHES: vec4<f32> = vec4<f32>(0.18, 0.32, 0.7, 0.86);

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn cloud_remap(v: f32, lo: f32, hi: f32) -> f32 {
    return (v - lo) / max(hi - lo, 1e-5);
}

fn cloud_hg(c: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (4.0 * PI * pow(max(1.0 + g2 - 2.0 * g * c, 1e-4), 1.5));
}

// The dual-lobe phase with both lobes' asymmetry scaled by `k` (the octaves').
fn cloud_phase(c: f32, k: f32) -> f32 {
    return mix(cloud_hg(c, clouds.phase.x * k), cloud_hg(c, clouds.phase.y * k), clouds.phase.z);
}

// A layer's point in its own turned, drifting domain (units).
fn layer_domain(li: u32, xy: vec2<f32>) -> vec2<f32> {
    let l = clouds.layers[li];
    let q = xy + l.offset.xy;
    return vec2<f32>(l.motion.z * q.x - l.motion.w * q.y, l.motion.w * q.x + l.motion.z * q.y);
}

// How much of the elements a patch holds by the weather map's b (0 in its clear patches).
fn element_present(b: f32) -> f32 {
    return smoothstep(ELEMENT_PATCHES.x - 0.08, ELEMENT_PATCHES.x - 0.03, b);
}

// A volume layer's density at p (the extinction per unit; 0 outside its slab) and its height in the slab (0 base, 1 top).
// `detail`: the detail volume's erosion; `lod`: the shape volume's mip level.
fn cloud_density(li: u32, p: vec3<f32>, detail: bool, lod: f32, spread: f32) -> vec2<f32> {
    let l = clouds.layers[li];
    let h = (p.z - l.slab.x) / max(l.slab.y - l.slab.x, 1e-3);
    if h <= 0.0 || h >= 1.0 {
        return vec2<f32>(0.0, h);
    }
    let r = layer_domain(li, p.xy);
    let w = textureSampleLevel(weather_tex, repeat_samp, r * l.domain.x, 0.0);
    var cov = clamp(l.shape.x + (w.r - 0.5) * l.domain.z, 0.0, 1.0);
    // Along a slanted ray that crosses many clouds' columns, each takes less of the sky so that together they cover what one
    // column overhead does (1 - (1 - c)^spread): the far field thins out (smaller, sparser puffs) instead of piling into a wall.
    cov = 1.0 - pow(1.0 - cov, spread);
    if cov < 0.02 {
        return vec2<f32>(0.0, h);
    }
    let elements = l.form.w > 0.5;
    var profile: f32;
    if elements {
        // A thin sheet: rounded above and below, a little thicker where the weather map's kind says.
        let top = mix(0.7, 1.0, w.g);
        profile = smoothstep(0.0, 0.3, h) * (1.0 - smoothstep(top * 0.55, top, h));
    } else {
        // Cumulus: a soft flat base, a rounded top whose height the weather map's kind sets (low heaps to tall towers).
        let top = mix(0.5, 1.0, w.g);
        profile = smoothstep(0.0, 0.07, h) * (1.0 - smoothstep(top * 0.5, top, h));
    }
    if profile <= 0.0 {
        return vec2<f32>(0.0, h);
    }
    let z = p.z + l.offset.z;
    let s = textureSampleLevel(shape_tex, repeat_samp, vec3<f32>(r, z) * l.shape.z, lod);
    let erosion = s.g * 0.625 + s.b * 0.25 + s.a * 0.125;
    var base = clamp(cloud_remap(s.r, erosion - 1.0, 1.0), 0.0, 1.0);
    if elements {
        // The fine cells are the elements (round heads with gaps between them, smaller than a row is wide): coarser in some patches,
        // finer in others (the weather map's kind), lined up in rows across the layer's domain whose phase the shape bends (billows,
        // not stripes) and the weather map bends and turns over kilometres. The coarse cells' low places break the rows into runs of
        // a few elements to many, out of step row to row. The weather map's b leaves patches clear of the layer and others of loose
        // elements in no rows.
        let cells = mix(s.b, s.a, 0.5 * smoothstep(0.35, 0.65, w.g));
        let k = l.shape.z * ELEMENT_ROWS;
        let rows = 0.5 + 0.5 * cos(2.0 * PI * (r.y * k + 0.9 * s.r + ELEMENT_BEND * (w.a - 0.5)));
        let breaks = smoothstep(ELEMENT_BREAKS.x, ELEMENT_BREAKS.y, s.g);
        let lined = smoothstep(ELEMENT_PATCHES.x, ELEMENT_PATCHES.y, w.b)
            * (1.0 - smoothstep(ELEMENT_PATCHES.z, ELEMENT_PATCHES.w, w.b));
        let present = element_present(w.b);
        let pattern = mix(1.0, rows, ELEMENT_ROW_SHARE * lined) * mix(1.0, breaks, lined);
        base = mix(base, clamp(cloud_remap(cells, 0.25, 0.95), 0.0, 1.0), 0.65) * pattern * present;
    }
    var d = clamp(cloud_remap(base * profile, 1.0 - cov, 1.0), 0.0, 1.0) * cov;
    // The edges' contrast: the thin fringe cut away and the rest stretched back, more toward the tops (sharp heads, softer bases).
    let rise = smoothstep(0.05, 0.45, h);
    d = clamp(cloud_remap(d, mix(l.form.x, l.form.y, rise), 1.0), 0.0, 1.0);
    if d <= 0.0 {
        return vec2<f32>(0.0, h);
    }
    if detail {
        let n = textureSampleLevel(detail_tex, repeat_samp, vec3<f32>(r, z) * l.shape.w, lod).rgb;
        let f = n.r * 0.625 + n.g * 0.25 + n.b * 0.125;
        let m = mix(f, 1.0 - f, clamp(h * 3.0, 0.0, 1.0));
        d = clamp(cloud_remap(d, m * l.domain.y, 1.0), 0.0, 1.0);
    }
    // Denser toward the tops (bright, defined heads over soft bases).
    d = d * (1.0 + l.form.z * rise);
    return vec2<f32>(d * l.shape.y, h);
}

// A sheet layer's density (0..1) at xy: its fibres where its patches' coverage lets them show, eroded by finer fibres.
fn sheet_density(li: u32, xy: vec2<f32>, detail: bool, lod: f32) -> f32 {
    let l = clouds.layers[li];
    let r = layer_domain(li, xy);
    let c = textureSampleLevel(cirrus_tex, repeat_samp, r * l.shape.z, lod).rgb;
    let blot = textureSampleLevel(cirrus_tex, repeat_samp, r * l.domain.x + vec2<f32>(0.37, 0.61), 0.0).g;
    let cov = clamp(l.shape.x + (blot - 0.5) * l.domain.z, 0.0, 1.0);
    if cov < 0.02 {
        return 0.0;
    }
    let fibre = mix(c.r, c.b, 0.35);
    var d = clamp(cloud_remap(fibre, 1.0 - cov, 1.0), 0.0, 1.0);
    if detail && d > 0.0 {
        let lod_detail = lod + log2(max(l.shape.w / max(l.shape.z, 1e-9), 1.0));
        let f = textureSampleLevel(cirrus_tex, repeat_samp, r * l.shape.w + vec2<f32>(0.11, 0.83), lod_detail).r;
        d = clamp(cloud_remap(d, (1.0 - f) * l.domain.y, 1.0), 0.0, 1.0);
    }
    d = d * d * (3.0 - 2.0 * d);
    // Under an overcast dense cirrus spreads into a veil: past CIRRUS_VEIL.x of coverage its fibres give way to an even sheet of half
    // their density (a dense sheet of solid fibres drew long bright streaks that fanned out in perspective overhead).
    let veil = smoothstep(CIRRUS_VEIL.x, CIRRUS_VEIL.y, cov) * CIRRUS_VEIL.z * clamp(clouds.weather.x, 0.0, 1.0);
    return mix(d, CIRRUS_VEIL.w * cov, veil);
}

// A layer's mean optical depth straight up through it around xy, as a lower layer's sun sees it: the sun's ray crosses a layer
// along a stretch many of its elements long (kilometres at a low sun), so what it takes is the layer's local coverage, not one
// element (a single element's column there would throw a noisy, unrelated shadow on every texel). A volume: its weather map's
// coverage, the share of its slab its elements fill there (about the coverage again) over its thickness; a sheet: its fibres
// blurred over that stretch.
fn layer_column(li: u32, xy: vec2<f32>) -> f32 {
    let l = clouds.layers[li];
    if l.slab.z > 0.5 {
        return sheet_density(li, xy, false, 4.0) * l.shape.y;
    }
    let r = layer_domain(li, xy);
    let w = textureSampleLevel(weather_tex, repeat_samp, r * l.domain.x, 2.0).rb;
    var cov = clamp(l.shape.x + (w.x - 0.5) * l.domain.z, 0.0, 1.0);
    if l.form.w > 0.5 {
        // The elements' clear patches.
        cov *= element_present(w.y);
    }
    return cov * cov * l.shape.y * (l.slab.y - l.slab.x) * 0.5;
}

// The share of the sun's light that the layers above layer `li` leave at p (their columns where the sun's ray crosses them).
fn shade_above(li: u32, p: vec3<f32>) -> f32 {
    let s = clouds.sun.xyz;
    if s.z <= 0.02 {
        return 1.0;
    }
    let n = u32(clouds.count.x);
    var od = 0.0;
    let slant = 1.0 / max(s.z, 1.0 / clouds.under.w);
    for (var j = li + 1u; j < n; j = j + 1u) {
        let l = clouds.layers[j];
        let mid = 0.5 * (l.slab.x + l.slab.y);
        let q = p.xy + s.xy * ((mid - p.z) / s.z);
        od = od + layer_column(j, q) * slant;
    }
    return exp(-od * clouds.under.z);
}

// The optical depth from p toward the sun through layer `li`: the light march in doubling steps (no detail), until it leaves the
// slab. Samples far from the eye (`dist`) take a step or two fewer (their light's detail is under a texel), and so do samples the
// view sees only through cloud already (`seen`, the view's transmittance to them: their share of the light is small). Past the near steps
// (the cloud's own shape) a low sun's long path through the deck shades less: the light scattered round the neighbours' edges
// still arrives, so a low sun's sunlit sides stay bright instead of every cloud lying in another's shadow.
fn sun_depth(li: u32, p: vec3<f32>, lod: f32, dist: f32, cos_sun: f32, spread: f32, seen: f32) -> f32 {
    let l = clouds.layers[li];
    let s = clouds.sun.xyz;
    var od = 0.0;
    var t = 0.0;
    var stride = clouds.phase.w;
    let fewer = u32(clamp(dist / 9000.0, 0.0, 2.0)) + select(0u, 1u, seen < DEEP_SEEN.x) + select(0u, 1u, seen < DEEP_SEEN.y);
    let n = max(u32(l.domain.w), 2u + fewer) - fewer;
    let far = mix(clouds.reach.x, 1.0, max(clamp(s.z, 0.0, 1.0), smoothstep(clouds.reach.z, clouds.reach.w, cos_sun)));
    let near = u32(clouds.reach.y);
    for (var k = 0u; k < n; k = k + 1u) {
        // The last step reaches as far as two more would have (one sample for the far part: cheaper, about the same shadow).
        if k + 1u == n {
            stride = stride * 3.0;
        }
        let q = p + s * (t + 0.5 * stride);
        if q.z >= l.slab.y || q.z <= l.slab.x {
            break;
        }
        let w = select(far, 1.0, k < near);
        od = od + cloud_density(li, q, false, lod + f32(k) * 0.5, spread).x * stride * w;
        t = t + stride;
        stride = stride * 2.0;
    }
    return od;
}

// The sky's light the frame's clouds take (the same for every layer): above, and the undersides' (the horizon's and the ground's).
struct Ambient {
    top: vec3<f32>,
    bottom: vec3<f32>,
};

fn ambient() -> Ambient {
    var a: Ambient;
    if clouds.lod.w > 0.5 {
        // Our own sky: the atmosphere's light above (from 35 degrees up) and at the horizon (2 degrees), round the azimuths.
        var up = vec3<f32>(0.0);
        var low = vec3<f32>(0.0);
        for (var k = 0u; k < 4u; k = k + 1u) {
            let az = (f32(k) + 0.5) * PI / 4.0;
            up = up + atmosphere_at(az, 0.6);
            low = low + atmosphere_at(az, 0.035);
        }
        // (The atmosphere's light as the sky draws it: the same scale as the server's box it stands in for.)
        a.top = up * 0.25;
        a.bottom = (low * 0.25 + 0.5 * a.top) * clouds.under.y;
        // Under an overcast the light round the clouds is their own, the sun's scattered through them: no colour.
        let k = clamp(clouds.weather.x, 0.0, 1.0);
        a.top = mix(a.top, vec3<f32>(sky_luminance(a.top)), k);
        a.bottom = mix(a.bottom, vec3<f32>(sky_luminance(a.bottom)), k);
    } else {
        a.top = clouds.amb_top.rgb;
        a.bottom = clouds.amb_bottom.rgb;
    }
    return a;
}

// The sun's light at a layer of radius `r` (km): through the atmosphere toward it, over its light overhead at the eye (the server's
// sun's colour kept at noon); none once the sun is under that height's horizon (a high layer keeps it after the low deck has lost it).
fn sun_at(r: f32) -> vec3<f32> {
    let s = clouds.sun.xyz;
    let rb = sky.radii.x;
    let horizon = -sqrt(max(1.0 - (rb / r) * (rb / r), 0.0));
    if s.z <= horizon {
        return vec3<f32>(0.0);
    }
    let toward = textureSampleLevel(transmittance_lut, sky_linear, transmittance_uv(r, s.z), 0.0).rgb;
    let overhead = textureSampleLevel(transmittance_lut, sky_linear, transmittance_uv(sky.radii.z, 1.0), 0.0).rgb;
    var tint = toward / max(overhead, vec3<f32>(1e-4));
    let m = max(tint.r, max(tint.g, tint.b));
    if clouds.weather.y > 0.5 {
        // The look's own sunlight: the air's transmittance whole (a low sun's light dims as it reddens).
        tint = min(tint, vec3<f32>(1.0));
    } else {
        // The colour only (its brightest channel kept): the server's sun keeps its strength, the low sun's light turns peach and
        // then red as the air's long path takes the blue.
        tint = mix(vec3<f32>(1.0), tint / max(m, 1e-4), clouds.octaves.w);
    }
    let fade = smoothstep(horizon, horizon + HORIZON_FADE, s.z);
    return clouds.sun_light.rgb * tint * fade;
}

// The light a layer's samples take: the sun's (at its height), the sky's above and its undersides'.
struct Lighting {
    sun: vec3<f32>,
    top: vec3<f32>,
    bottom: vec3<f32>,
};

fn layer_lighting(li: u32, a: Ambient) -> Lighting {
    let l = clouds.layers[li];
    var o: Lighting;
    o.sun = sun_at(l.light.z);
    o.top = a.top;
    // The undersides: the sky's and the sunlit ground's; over a lower deck, its sunlit tops in its share of the sky.
    let ground = clouds.under.x * (o.sun * max(clouds.sun.z, 0.0) + 0.5 * a.top);
    let deck = 0.6 * o.sun * max(clouds.sun.z, 0.0) + 0.4 * a.top;
    o.bottom = a.bottom + mix(ground, deck, l.light.w);
    return o;
}

// The light a volume sample of extinction `sigma` at height `h` scatters toward the eye (per unit of its in-scatter); `above` the
// share of the sun the layers above leave.
fn cloud_light(li: u32, p: vec3<f32>, sigma: f32, h: f32, cos_sun: f32, lod: f32, dist: f32, lg: Lighting, above: f32, spread: f32, seen: f32) -> vec3<f32> {
    let l = clouds.layers[li];
    var sun = vec3<f32>(0.0);
    let lit = lg.sun * above;
    if max(lit.r, max(lit.g, lit.b)) > 0.0 {
        let od = sun_depth(li, p, lod, dist, cos_sun, spread, seen);
        // The multiple-scattering octaves.
        var ms = 0.0;
        var a = 1.0;
        var b = 1.0;
        var c = 1.0;
        let n = u32(clouds.sun_light.w);
        for (var i = 0u; i < n; i = i + 1u) {
            ms = ms + a * cloud_phase(cos_sun, c) * exp(-od * b);
            a = a * clouds.octaves.x;
            b = b * clouds.octaves.y;
            c = c * clouds.octaves.z;
        }
        // The light scattered many times inside (a diffuse share that decays slower: the clouds' bodies glow with the sun).
        ms = ms + clouds.amb_bottom.w / (4.0 * PI) * exp(-od * clouds.tint.w);
        // Beer-powder: the sides away from the sun darken where the light has not yet scattered enough to reach the eye.
        let powder = 1.0 - exp(-2.0 * sigma * POWDER_DEPTH);
        let pw = mix(1.0, powder, clouds.sun.w * (0.5 - 0.5 * cos_sun));
        sun = lit * (PI * ms * pw * l.light.x);
    }
    let up = sqrt(clamp(h, 0.0, 1.0));
    // The sky's light is shaded by the cloud around the sample: lower in the cloud and deeper in its denser parts.
    let occl = mix(clouds.amb_top.w, 1.0, up) * exp(-sigma * AMBIENT_DEPTH);
    let amb = mix(lg.bottom, lg.top, up) * (l.light.y * occl);
    return sun + amb;
}

struct Marched {
    // rgb: the light toward the eye (over what is behind it), a: the transmittance.
    c: vec4<f32>,
    // The step-weighted mean distance of what was seen (the reach when nothing was).
    dist: f32,
};

// The air between the eye and a layer's light `c` (covering `cover` of what is behind) at `dist`: with our own sky toward the sky
// behind; with the server's box faded toward it. Both fade out before the reach.
fn layer_air(li: u32, c: vec3<f32>, cover: f32, dist: f32, dir: vec3<f32>) -> vec4<f32> {
    let l = clouds.layers[li];
    let far = layer_far(l, clouds.layers[0].far.w, clouds.count.z, clouds.lod.w, dist);
    if clouds.lod.w > 0.5 {
        let air = exp(-dist / max(l.far.x, 1.0));
        // (Under the horizon, as the sky draws it there: a deck below the eye fades into the same colour.)
        let behind = atmosphere_drawn(dir);
        let lit = c * air + behind * cover * (1.0 - air);
        return vec4<f32>(lit * far, 1.0 - far * cover);
    }
    return vec4<f32>(c * far, 1.0 - far * cover);
}

// The share of a layer's cover at `dist` that the eye sees (layer_air's): both skies fade it out before its reach; the server's box
// also fades far clouds into itself (a layer that reaches farther, its haze in proportion). `deck_reach`: the lowest layer's reach,
// `box_fade` the box's far fade (units), `own_sky` 1 with the atmosphere's own sky.
fn layer_far(l: Layer, deck_reach: f32, box_fade: f32, own_sky: f32, dist: f32) -> f32 {
    let far = 1.0 - smoothstep(l.far.y, l.far.z, dist);
    if own_sky > 0.5 {
        return far;
    }
    let x = dist / max(box_fade * l.far.w / max(deck_reach, 1.0), 1.0);
    return exp(-x * x) * far;
}

fn march_volume(li: u32, o: vec3<f32>, dir: vec3<f32>, t0: f32, t1: f32, j: f32, a: Ambient) -> Marched {
    let l = clouds.layers[li];
    var out: Marched;
    let n = max(u32(l.slab.w), 1u);
    let cos_sun = dot(dir, clouds.sun.xyz);
    let detail = clouds.march.w > 0.5;
    let footprint = clouds.lod.x * clouds.march.z;
    let lit = layer_lighting(li, a);
    // A low ray crosses the slab on a long slant and would pile up many clouds into a wall near the horizon (the deck reading
    // as a dome): past a slant of thin.x times the thickness its coverage spreads thinner (each cloud on it takes less of the sky,
    // thin.y) and its density thins (thin.z), so the far field covers about what the sky overhead does.
    let slant = (t1 - t0) / max((l.slab.y - l.slab.x) * clouds.thin.x, 1.0);
    // (With the server's box the far field already fades into the box by its distance: no spread there.)
    let spread = select(1.0, pow(clamp(1.0 / max(slant, 1e-3), 0.0, 1.0), clouds.thin.y), clouds.lod.w > 0.5);
    let thin = pow(clamp(1.0 / max(slant, 1e-3), 0.0, 1.0), clouds.thin.z);
    // The layers above shade this one's sun: their columns at the segment's ends, between them by the step.
    let above0 = shade_above(li, o + dir * t0);
    let above1 = shade_above(li, o + dir * t1);
    var tr = 1.0;
    var light = vec3<f32>(0.0);
    var seen = 0.0;
    var at = 0.0;
    // A segment that starts at the eye (the eye in the slab) or near it takes its steps closer together near the eye (c from 0 for a
    // segment that starts its own length or more away to 1 at the eye): the cloud about the eye is drawn in steps
    // as short as its features, the far part in longer ones. The same steps as before for an eye under the slab.
    let span = t1 - t0;
    if clouds.weather.z > 0.0 {
        return march_refined(li, o, dir, t0, t1, j, lit, above0, above1, spread, thin, cos_sun);
    }
    let c = clamp(1.0 - t0 / max(span, 1e-3), 0.0, 1.0);
    for (var i = 0u; i < n; i = i + 1u) {
        let f = (f32(i) + j) / f32(n);
        // (t0 + L ((1 - c) f + c f^2): even steps for c 0, closer near the eye for c up to 1.)
        let t = t0 + span * f * (1.0 - c + c * f);
        let dt = span * (1.0 - c + 2.0 * c * max(f, 0.5 / f32(n))) / f32(n);
        let p = o + dir * t;
        let lod = max(log2(max(t * footprint, 1e-3) / l.offset.w), 0.0);
        // (Past the far fade's start the detail is under a texel and the cloud fading: the shape alone.)
        var d = cloud_density(li, p, detail && t < l.far.y, lod, spread);
        d.x = d.x * thin;
        if d.x > 0.0 {
            let k = 1.0 - exp(-d.x * dt);
            light = light + tr * k * cloud_light(li, p, d.x, d.y, cos_sun, lod, t, lit, mix(above0, above1, f), spread, tr);
            at = at + tr * k * t;
            seen = seen + tr * k;
            tr = tr * (1.0 - k);
            if tr < 0.01 {
                break;
            }
        }
    }
    out.dist = select(l.far.w, at / max(seen, 1e-6), seen > 1e-4);
    out.c = layer_air(li, light * clouds.tint.rgb, 1.0 - tr, out.dist, dir);
    return out;
}

// The look's refined march (weather.z the fine step, units; weather.w the most samples as a multiple of the layer's steps): the
// layer's steps through clear air, and where one finds cloud, back a step and on through the cloud in fine steps (no shorter than
// the pixel's footprint there) until FINE_EMPTY in a row find none. The coarse march's first sample in a cloud lies a long step
// deep, where the sun's light has gone: the sunlit shell, a few fine steps thick, is what gives a cumulus its lit side, its dark
// side and its bright rim.
fn march_refined(li: u32, o: vec3<f32>, dir: vec3<f32>, t0: f32, t1: f32, j: f32, lit: Lighting, above0: f32, above1: f32, spread: f32, thin: f32, cos_sun: f32) -> Marched {
    let l = clouds.layers[li];
    var out: Marched;
    let n = max(u32(l.slab.w), 1u);
    let detail = clouds.march.w > 0.5;
    let footprint = clouds.lod.x * clouds.march.z;
    let span = t1 - t0;
    let coarse = span / f32(n);
    let budget = u32(f32(n) * max(clouds.weather.w, 1.0));
    var tr = 1.0;
    var light = vec3<f32>(0.0);
    var seen = 0.0;
    var at = 0.0;
    var t = t0 + coarse * j;
    var fine_mode = false;
    var found = false;
    var empty = 0u;
    for (var i = 0u; i < budget; i = i + 1u) {
        if t >= t1 {
            break;
        }
        let fine = clamp(max(clouds.weather.z, t * footprint), 1.0, coarse);
        let p = o + dir * t;
        let lod = max(log2(max(t * footprint, 1e-3) / l.offset.w), 0.0);
        var d = cloud_density(li, p, detail && t < l.far.y, lod, spread);
        d.x = d.x * thin;
        if !fine_mode {
            if d.x > 0.0 {
                fine_mode = true;
                found = false;
                empty = 0u;
                t = max(t - coarse + fine * j, t0);
            } else {
                t = t + coarse;
            }
            continue;
        }
        if d.x > 0.0 {
            found = true;
            empty = 0u;
            let dt = min(fine, t1 - t);
            let k = 1.0 - exp(-d.x * dt);
            let f = clamp((t - t0) / max(span, 1e-3), 0.0, 1.0);
            light = light + tr * k * cloud_light(li, p, d.x, d.y, cos_sun, lod, t, lit, mix(above0, above1, f), spread, tr);
            at = at + tr * k * t;
            seen = seen + tr * k;
            tr = tr * (1.0 - k);
            if tr < 0.01 {
                break;
            }
        } else {
            // (Before the cloud is reached again, as many fine steps as the coarse one took.)
            empty = empty + 1u;
            if empty >= select(u32(ceil(coarse / fine)) + 2u, FINE_EMPTY, found) {
                fine_mode = false;
            }
        }
        t = t + fine;
    }
    out.dist = select(l.far.w, at / max(seen, 1e-6), seen > 1e-4);
    out.c = layer_air(li, light * clouds.tint.rgb, 1.0 - tr, out.dist, dir);
    return out;
}

fn draw_sheet(li: u32, o: vec3<f32>, dir: vec3<f32>, a: Ambient) -> Marched {
    let l = clouds.layers[li];
    var out: Marched;
    out.c = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    out.dist = l.far.w;
    let t = (l.slab.x - o.z) / dir.z;
    if t <= 0.0 || t >= l.far.w {
        return out;
    }
    let p = o + dir * t;
    let lod = max(log2(max(t * clouds.lod.x * clouds.march.z, 1e-3) / l.offset.w), 0.0);
    let d = sheet_density(li, p.xy, clouds.march.w > 0.5, lod);
    if d <= 0.0 {
        return out;
    }
    let od = d * l.shape.y;
    // Seen aslant the sheet is thicker (to a point); it fades in away from its edge-on view (from under it above the horizon, from
    // over it under).
    let el = abs(dir.z);
    let cover = (1.0 - exp(-od / max(el, SHEET_SLANT))) * smoothstep(SHEET_LOW, SHEET_HIGH, el);
    let lit = layer_lighting(li, a);
    let s = clouds.sun.xyz;
    let cos_sun = dot(dir, s);
    // Thin ice: one scattering, a strong forward lobe and a weak back lobe, the sun's light dimmed a little by the sheet itself.
    let phase = mix(cloud_hg(cos_sun, ICE_BACK), cloud_hg(cos_sun, ICE_FORWARD), ICE_FORWARD_SHARE);
    let self_shade = 0.5 + 0.5 * exp(-od / max(abs(s.z), 0.1));
    let sun = lit.sun * (PI * phase * l.light.x * self_shade);
    let amb = mix(lit.bottom, lit.top, 0.65) * l.light.y;
    out.dist = t;
    out.c = layer_air(li, (sun + amb) * clouds.tint.rgb * cover, cover, t, dir);
    return out;
}

// The layers composited front to back so far: their light, the transmittance through them, and the seen-weighted sums of their
// distance and motion.
struct Composite {
    light: vec3<f32>,
    tr: f32,
    seen: f32,
    at: f32,
    moved: vec2<f32>,
};

// Layer `li` behind what `acc` holds.
// Where a ray from `o` along `dir` is inside a volume layer's slab (from, to: units along it; to <= from where it misses): from the
// eye when the eye is in it, upward through the layers above, downward through those below; no farther than the layer's reach,
// and a stretch of no more than SEGMENT_MAX.
fn slab_segment(l: Layer, o: vec3<f32>, dir: vec3<f32>) -> vec2<f32> {
    var t0 = 0.0;
    var t1 = l.far.w;
    if abs(dir.z) < 1e-6 {
        if o.z <= l.slab.x || o.z >= l.slab.y {
            return vec2<f32>(0.0, -1.0);
        }
    } else {
        let ta = (l.slab.x - o.z) / dir.z;
        let tb = (l.slab.y - o.z) / dir.z;
        t0 = max(min(ta, tb), 0.0);
        t1 = min(max(ta, tb), l.far.w);
    }
    return vec2<f32>(t0, min(t1, t0 + SEGMENT_MAX));
}

fn add_layer(li: u32, acc: ptr<function, Composite>, o: vec3<f32>, dir: vec3<f32>, j: f32, a: Ambient) {
    if (*acc).tr < 0.01 {
        return;
    }
    let l = clouds.layers[li];
    var m: Marched;
    m.c = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    m.dist = l.far.w;
    if l.slab.z > 0.5 {
        m = draw_sheet(li, o, dir, a);
    } else {
        let s = slab_segment(l, o, dir);
        if s.y > s.x {
            m = march_volume(li, o, dir, s.x, s.y, j, a);
        }
    }
    let tr = (*acc).tr;
    let w = tr * (1.0 - m.c.a);
    (*acc).light = (*acc).light + tr * m.c.rgb;
    (*acc).at = (*acc).at + w * m.dist;
    (*acc).moved = (*acc).moved + w * l.motion.xy;
    (*acc).seen = (*acc).seen + w;
    (*acc).tr = tr * m.c.a;
}

struct MarchOut {
    @location(0) c: vec4<f32>,
    // r: the distance over DIST_SCALE, gb: the layers' motion since the last frame (seen-weighted).
    @location(1) dist: vec4<f32>,
};

@fragment
fn fs_march(@builtin(position) frag: vec4<f32>) -> MarchOut {
    let texel = floor(frag.xy);
    let px = (texel + 0.5) * clouds.march.z;
    let ndc = vec2<f32>(px.x * frame.screen.z * 2.0 - 1.0, 1.0 - px.y * frame.screen.w * 2.0);
    let far = frame.inv_unjit_view_proj * vec4<f32>(ndc, 1e-6, 1.0);
    let eye = frame.eye.xyz;
    let dir = normalize(far.xyz / far.w - eye);
    let o = vec3<f32>(eye.xy, clouds.count.y);
    var acc: Composite;
    acc.light = vec3<f32>(0.0);
    acc.tr = 1.0;
    acc.seen = 0.0;
    acc.at = 0.0;
    acc.moved = vec2<f32>(0.0);
    // (A ray down from under the lowest layer meets none.)
    if dir.z > 0.0 || o.z > clouds.layers[0].slab.x {
        let b = textureLoad(blue_tex, vec2<i32>(texel) % vec2<i32>(textureDimensions(blue_tex)), 0).r;
        let j = fract(b + clouds.march.x * GOLDEN);
        let a = ambient();
        let n = u32(clouds.count.x);
        // Front to back: up through the layers from the lowest, down from the highest (a layer the ray does not cross adds
        // nothing). Each layer by its own constant index (the compiler then reads its uniform fields directly in the hot loops; a
        // loop over the layers down cost 0.1 ms more on every ray).
        if dir.z >= 0.0 {
            if n > 0u {
                add_layer(0u, &acc, o, dir, j, a);
            }
            if n > 1u {
                add_layer(1u, &acc, o, dir, j, a);
            }
            if n > 2u {
                add_layer(2u, &acc, o, dir, j, a);
            }
        } else {
            if n > 2u {
                add_layer(2u, &acc, o, dir, j, a);
            }
            if n > 1u {
                add_layer(1u, &acc, o, dir, j, a);
            }
            if n > 0u {
                add_layer(0u, &acc, o, dir, j, a);
            }
        }
    }
    let light = acc.light;
    let tr = acc.tr;
    let seen = acc.seen;
    let at = acc.at;
    var moved = acc.moved;
    var cur = vec4<f32>(light, tr);
    var dist = clouds.layers[0].far.w;
    if seen > 1e-4 {
        dist = at / seen;
        moved = moved / seen;
    }
    // Never a non-finite value into the history.
    if !(all(cur == cur) && all(abs(cur) < vec4<f32>(1e8))) {
        cur = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    if !(dist == dist && abs(dist) < 1e7 && all(moved == moved) && all(abs(moved) < vec2<f32>(1e4))) {
        dist = clouds.layers[0].far.w;
        moved = vec2<f32>(0.0);
    }
    var out: MarchOut;
    out.c = cur;
    out.dist = vec4<f32>(dist / DIST_SCALE, moved, 1.0);
    return out;
}

// The Catmull-Rom weights of a fraction (each axis).
fn catmull_rom(f: vec2<f32>) -> array<vec2<f32>, 4> {
    let f2 = f * f;
    let f3 = f2 * f;
    return array<vec2<f32>, 4>(
        -0.5 * f3 + f2 - 0.5 * f,
        1.5 * f3 - 2.5 * f2 + 1.0,
        -1.5 * f3 + 2.0 * f2 + 0.5 * f,
        0.5 * f3 - 0.5 * f2,
    );
}

// The history at uv (0..1 of its texels' span) through a Catmull-Rom filter (16 loads), held within its four nearest texels (no
// ringing past them).
fn history_at(uv: vec2<f32>) -> vec4<f32> {
    let dims = vec2<i32>(textureDimensions(history_tex));
    let p = uv * vec2<f32>(dims) - 0.5;
    let base = floor(p);
    let w = catmull_rom(p - base);
    var sum = vec4<f32>(0.0);
    var lo = vec4<f32>(1e9);
    var hi = vec4<f32>(-1e9);
    for (var j = 0; j < 4; j = j + 1) {
        for (var i = 0; i < 4; i = i + 1) {
            let q = clamp(vec2<i32>(base) + vec2<i32>(i - 1, j - 1), vec2<i32>(0), dims - 1);
            let v = textureLoad(history_tex, q, 0);
            sum = sum + v * (w[i].x * w[j].y);
            if (i == 1 || i == 2) && (j == 1 || j == 2) {
                lo = min(lo, v);
                hi = max(hi, v);
            }
        }
    }
    return clamp(sum, lo, hi);
}

@fragment
fn fs_resolve(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let c = resolve_clouds;
    let texel = vec2<i32>(floor(frag.xy));
    let dims = vec2<i32>(textureDimensions(current_tex));
    let cur = textureLoad(current_tex, texel, 0);
    if c.count.w < 0.5 {
        return cur;
    }
    // The current neighbourhood's mean and spread.
    var sum1 = vec4<f32>(0.0);
    var sum2 = vec4<f32>(0.0);
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let v = textureLoad(current_tex, clamp(texel + vec2<i32>(i, j), vec2<i32>(0), dims - 1), 0);
            sum1 = sum1 + v;
            sum2 = sum2 + v * v;
        }
    }
    sum1 = sum1 / 9.0;
    let sd = sqrt(max(sum2 / 9.0 - sum1 * sum1, vec4<f32>(0.0)));
    let lo = sum1 - sd * c.lod.z;
    let hi = sum1 + sd * c.lod.z;
    // The texel's point as last frame saw it.
    let px = (vec2<f32>(texel) + 0.5) * c.march.z;
    let ndc = vec2<f32>(px.x * frame.screen.z * 2.0 - 1.0, 1.0 - px.y * frame.screen.w * 2.0);
    let far = frame.inv_unjit_view_proj * vec4<f32>(ndc, 1e-6, 1.0);
    let eye = frame.eye.xyz;
    let dir = normalize(far.xyz / far.w - eye);
    let dm = textureLoad(dist_tex, texel, 0);
    let back = eye + dir * (dm.r * DIST_SCALE) - vec3<f32>(dm.g, dm.b, 0.0);
    let clip = frame.prev_view_proj * vec4<f32>(back, 1.0);
    if clip.w <= 1e-4 {
        return cur;
    }
    let n = clip.xy / clip.w;
    let uv = vec2<f32>(0.5 + 0.5 * n.x, 0.5 - 0.5 * n.y) * c.uvmap.xy;
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return cur;
    }
    let h = clamp(history_at(uv), lo, hi);
    // A texel that moved far on screen (a fast turn) leans less on its history: a lagging history is the larger error there.
    let moved = length((uv - (vec2<f32>(texel) + 0.5) / vec2<f32>(dims)) * vec2<f32>(dims));
    let weight = c.march.y * (1.0 - SPIN_EASE * smoothstep(1.0, 8.0, moved));
    var out = mix(cur, h, weight);
    if !(all(out == out) && all(abs(out) < vec4<f32>(1e8))) {
        out = cur;
    }
    return out;
}

@fragment
fn fs_shadow(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let s = clouds.sun.xyz;
    if s.z < 0.05 || clouds.count.x < 0.5 {
        return vec4<f32>(1.0, 0.0, 0.0, 1.0);
    }
    let l = clouds.layers[0];
    let uv = (floor(frag.xy) + 0.5) / max(clouds.lod.y, 1.0);
    let q = clouds.map.xy + (uv - 0.5) * clouds.map.z;
    let o = vec3<f32>(q, l.slab.x);
    var od = 0.0;
    if l.slab.z > 0.5 {
        od = layer_column(0u, q) / s.z;
    } else {
        let length = min((l.slab.y - l.slab.x) / s.z, 8000.0);
        let n = max(u32(clouds.map.w), 1u);
        let dt = length / f32(n);
        for (var i = 0u; i < n; i = i + 1u) {
            od = od + cloud_density(0u, o + s * ((f32(i) + 0.5) * dt), false, 1.0, 1.0).x * dt;
        }
    }
    return vec4<f32>(exp(-od) * shade_above(0u, o), 0.0, 0.0, 1.0);
}

// The sun's light through the clouds as the eye sees it (hd::clouds::CLOUD_SUN): SUN_RAYS rays across a disc about the
// sun (its radius the uniform's thin.w), each layer's segment marched by the march's rules (its spread, its thinning, its far fade;
// no light) in SUN_SEGMENTS parts, one invocation a part (fs_sun_rays: a texel per ray and part across, one row per layer), then
// their mean (fs_sun_mean, one texel). The forward passes, the sky and the volumetric light read it at a low sun (cloudshadow.wgsl).
const SUN_RAYS: u32 = 16u;
const SUN_SEGMENTS: u32 = 4u;
const SUN_SEGMENT_STEPS: u32 = 8u;
// A part's distance-weighted sum is kept times this (half floats).
const SUN_AT_SCALE: f32 = 1.0 / 1024.0;

@group(1) @binding(15) var sun_rays_tex: texture_2d<f32>;

// Ray `k`'s direction: a Vogel disc of radius `spread` about the sun.
fn sun_ray_dir(k: u32, s: vec3<f32>, spread: f32) -> vec3<f32> {
    let side = normalize(select(vec3<f32>(1.0, 0.0, 0.0), cross(s, vec3<f32>(0.0, 0.0, 1.0)), abs(s.z) < 0.999));
    let up = cross(side, s);
    let r = spread * sqrt((f32(k) + 0.5) / f32(SUN_RAYS));
    let a = f32(k) * 2.39996323;
    return normalize(s + side * (r * cos(a)) + up * (r * sin(a)));
}

// One part of a layer's segment along a ray toward the sun: r its transmittance, g the share it hides of what enters it, b that
// share's distance-weighted sum (times SUN_AT_SCALE).
@fragment
fn fs_sun_rays(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let texel = vec2<u32>(floor(frag.xy));
    let k = texel.x / SUN_SEGMENTS;
    let part = texel.x % SUN_SEGMENTS;
    let li = texel.y;
    let none = vec4<f32>(1.0, 0.0, 0.0, 1.0);
    if li >= u32(clouds.count.x) || li >= 3u {
        return none;
    }
    let dir = sun_ray_dir(k, clouds.sun.xyz, clouds.thin.w);
    if dir.z <= 0.002 {
        return none;
    }
    let o = vec3<f32>(frame.eye.xy, clouds.count.y);
    let l = clouds.layers[li];
    // The rays' spacing: each stands for its share of the disc.
    let foot = clouds.thin.w / sqrt(f32(SUN_RAYS));
    let detail = clouds.march.w > 0.5;
    if l.slab.z > 0.5 {
        let t = (l.slab.x - o.z) / dir.z;
        if part != 0u || t <= 0.0 || t >= l.far.w {
            return none;
        }
        let lod = max(log2(max(t * foot, 1e-3) / l.offset.w), 0.0);
        let d = sheet_density(li, (o + dir * t).xy, detail, lod);
        let cover = (1.0 - exp(-d * l.shape.y / max(dir.z, SHEET_SLANT))) * smoothstep(SHEET_LOW, SHEET_HIGH, dir.z);
        return vec4<f32>(1.0 - cover, cover, cover * t * SUN_AT_SCALE, 1.0);
    }
    let seg = slab_segment(l, o, dir);
    let t0 = seg.x;
    let t1 = seg.y;
    if t1 <= t0 {
        return none;
    }
    let slant = (t1 - t0) / max((l.slab.y - l.slab.x) * clouds.thin.x, 1.0);
    let spread = select(1.0, pow(clamp(1.0 / max(slant, 1e-3), 0.0, 1.0), clouds.thin.y), clouds.lod.w > 0.5);
    let thin = pow(clamp(1.0 / max(slant, 1e-3), 0.0, 1.0), clouds.thin.z);
    let dt = (t1 - t0) / f32(SUN_SEGMENTS * SUN_SEGMENT_STEPS);
    var tr = 1.0;
    var seen = 0.0;
    var at = 0.0;
    for (var i = part * SUN_SEGMENT_STEPS; i < (part + 1u) * SUN_SEGMENT_STEPS; i = i + 1u) {
        let t = t0 + (f32(i) + 0.5) * dt;
        let lod = max(log2(max(t * foot, 1e-3) / l.offset.w), 0.0);
        let d = cloud_density(li, o + dir * t, detail && t < l.far.y, lod, spread).x * thin;
        if d > 0.0 {
            let a = 1.0 - exp(-d * dt);
            seen = seen + tr * a;
            at = at + tr * a * t;
            tr = tr * (1.0 - a);
            if tr < 0.01 {
                break;
            }
        }
    }
    return vec4<f32>(tr, seen, at * SUN_AT_SCALE, 1.0);
}

// The rays' mean transmittance through every layer, each layer's cover faded by its distance as the picture fades it.
@fragment
fn fs_sun_mean(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let c = resolve_clouds;
    let n = min(u32(c.count.x), 3u);
    var sum = 0.0;
    for (var k = 0u; k < SUN_RAYS; k = k + 1u) {
        var ray = 1.0;
        for (var li = 0u; li < n; li = li + 1u) {
            var tr = 1.0;
            var seen = 0.0;
            var at = 0.0;
            for (var part = 0u; part < SUN_SEGMENTS; part = part + 1u) {
                let v = textureLoad(sun_rays_tex, vec2<i32>(i32(k * SUN_SEGMENTS + part), i32(li)), 0);
                seen = seen + tr * v.g;
                at = at + tr * v.b;
                tr = tr * v.r;
            }
            let l = c.layers[li];
            let dist = select(l.far.w, at / (max(seen, 1e-6) * SUN_AT_SCALE), seen > 1e-4);
            ray = ray * (1.0 - layer_far(l, c.layers[0].far.w, c.count.z, c.lod.w, dist) * clamp(1.0 - tr, 0.0, 1.0));
        }
        sum = sum + ray;
    }
    var t = sum / f32(SUN_RAYS);
    if !(t == t) {
        t = 1.0;
    }
    return vec4<f32>(clamp(t, 0.0, 1.0), 0.0, 0.0, 1.0);
}
