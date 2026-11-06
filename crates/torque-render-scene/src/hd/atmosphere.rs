//! The physical sky as plain data:
//! a planet's atmosphere after Hillaire 2020 ("A Scalable and Production Ready Sky and Atmosphere Rendering Technique", EGSR), with
//! the Earth-like parameters published with Bruneton 2017's model (Rayleigh, Mie and an ozone layer), the three look-up tables the GPU
//! builds (their sizes, step counts and texel parameterisations, which the WGSL mirrors) and our own f64 CPU integrator of the same
//! equations (the reference the GPU tables are checked against, 2 % over 256 directions).
//!
//! **The equations.** Lengths are kilometres, the planet's centre the origin, z up. At a height h the medium scatters
//! `σs = βR e^(-h/HR) + βM e^(-h/HM)` and takes away `σt = σs + βM,abs e^(-h/HM) + βO max(0, 1 - |h - hO| / wO)`. The transmittance
//! along a segment is `exp(-∫σt)`. The light a view ray gathers is `∫ T(t) S(t) dt` with
//! `S = E [(σR pR(ν) + σM pM(ν)) Tsun V + (σR + σM) Ψms]`: `pR` Rayleigh's phase, `pM` Cornette and Shanks', `ν` the cosine between
//! the view and the sun, `Tsun` the transmittance toward the sun, `V` 0 where the planet hides the sun, `Ψms` Hillaire's multiple
//! scattering (an isotropic second order `L2` spread to every order: `Ψ = L2 / (1 - fms)`, with the ground's Lambertian albedo).
//! Each step integrates `S` analytically over its constant medium: `(S - S e^(-σt dt)) / σt`.
//!
//! **The tables** (GPU): the transmittance (256 x 64) over Bruneton's (r, μ) mapping, which crowds the texels near the horizon; the
//! multiple scattering (32 x 32) over (μsun, √h: crowded near the ground, where it changes fastest); the sky view (192 x 108) for the camera's height over the azimuth from the sun (0 to
//! π: the sky is symmetric about the sun's vertical plane) and the elevation, mapped quadratically around the horizon. The CPU
//! integrator takes the transmittance by direct integration (fine steps), its multiple scattering from its own finer table, and the
//! view rays in fine uniform steps.

use glam::DVec3;

/// The atmosphere's parameters (km, per km).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Atmosphere {
    pub bottom: f64,
    pub top: f64,
    pub rayleigh: Rgb,
    pub rayleigh_height: f64,
    pub mie_scattering: f64,
    pub mie_absorption: f64,
    pub mie_height: f64,
    pub mie_g: f64,
    pub ozone: Rgb,
    pub ozone_centre: f64,
    pub ozone_half_width: f64,
    pub ground_albedo: Rgb,
}

/// The Earth-like atmosphere (Bruneton 2017's published parameters; Hillaire 2020 uses the same).
pub const EARTH: Atmosphere = Atmosphere {
    bottom: 6360.0,
    top: 6460.0,
    rayleigh: [5.802e-3, 13.558e-3, 33.1e-3],
    rayleigh_height: 8.0,
    mie_scattering: 3.996e-3,
    mie_absorption: 0.444e-3,
    mie_height: 1.2,
    mie_g: 0.8,
    ozone: [0.650e-3, 1.881e-3, 0.085e-3],
    ozone_centre: 25.0,
    ozone_half_width: 15.0,
    ground_albedo: [0.3, 0.3, 0.3],
};

/// The camera's height above the ground (km): a world is small against the atmosphere, so the sky is that of one height.
pub const CAMERA_HEIGHT: f64 = 0.2;

/// The GPU's tables: sizes and step counts (the WGSL reads these from `sky.wgsl`'s constants; a test compares them).
pub const TRANSMITTANCE_SIZE: (u32, u32) = (256, 64);
pub const TRANSMITTANCE_STEPS: u32 = 64;
pub const MULTISCATTER_SIZE: u32 = 64;
pub const MULTISCATTER_DIRECTIONS: u32 = 128;
pub const MULTISCATTER_STEPS: u32 = 64;
pub const SKY_VIEW_SIZE: (u32, u32) = (192, 108);
pub const SKY_VIEW_STEPS: u32 = 48;
/// The multiple scattering table's sun cosine is `sinh(k t) / sinh(k)` of t in [-1, 1]: its texels crowded about the terminator.
pub const MULTISCATTER_CROWD: f64 = 2.0;

/// The multiple scattering table's sun cosine at x in [0, 1] (the GPU's `multiscatter_mu`).
pub fn multiscatter_mu(x: f64) -> f64 {
    (MULTISCATTER_CROWD * (2.0 * x - 1.0)).sinh() / MULTISCATTER_CROWD.sinh()
}

/// The CPU integrator's resolution.
const REF_TRANSMITTANCE_STEPS: usize = 1024;
const REF_VIEW_STEPS: usize = 2048;
const REF_MS_SIZE: (usize, usize) = (512, 64);
const REF_MS_DIRECTIONS: usize = 256;
const REF_MS_STEPS: usize = 192;
const REF_T_TABLE: (usize, usize) = (512, 256);

pub type Rgb = [f64; 3];

/// A texel coordinate's [0, 1] value with the texel centres at the ends (Bruneton 2017's `GetTextureCoordFromUnitRange`).
pub fn to_unit(x: f64, n: u32) -> f64 {
    0.5 / f64::from(n) + x * (1.0 - 1.0 / f64::from(n))
}

/// [`to_unit`]'s inverse.
pub fn from_unit(u: f64, n: u32) -> f64 {
    (u - 0.5 / f64::from(n)) / (1.0 - 1.0 / f64::from(n))
}

/// The scattering and extinction of the medium at height `h`.
#[derive(Debug, Clone, Copy)]
pub struct Medium {
    pub rayleigh: Rgb,
    pub mie: f64,
    pub extinction: Rgb,
}

impl Atmosphere {
    pub fn medium(&self, h: f64) -> Medium {
        let h = h.max(0.0);
        let dr = (-h / self.rayleigh_height).exp();
        let dm = (-h / self.mie_height).exp();
        let dozone = (1.0 - (h - self.ozone_centre).abs() / self.ozone_half_width).max(0.0);
        let rayleigh = DVec3::from(self.rayleigh) * dr;
        let mie = self.mie_scattering * dm;
        let mie_ext = (self.mie_scattering + self.mie_absorption) * dm;
        let extinction = rayleigh + mie_ext + DVec3::from(self.ozone) * dozone;
        Medium {
            rayleigh: rayleigh.to_array(),
            mie,
            extinction: extinction.to_array(),
        }
    }

    /// The distance from radius `r` along the cosine `mu` (from the zenith) to the top of the atmosphere.
    pub fn distance_to_top(&self, r: f64, mu: f64) -> f64 {
        let disc = r * r * (mu * mu - 1.0) + self.top * self.top;
        (-r * mu + disc.max(0.0).sqrt()).max(0.0)
    }

    /// The distance to the ground (when the ray meets it).
    pub fn distance_to_bottom(&self, r: f64, mu: f64) -> f64 {
        let disc = r * r * (mu * mu - 1.0) + self.bottom * self.bottom;
        (-r * mu - disc.max(0.0).sqrt()).max(0.0)
    }

    /// Whether the ray from `r` along `mu` meets the ground.
    pub fn hits_ground(&self, r: f64, mu: f64) -> bool {
        mu < 0.0 && r * r * (mu * mu - 1.0) + self.bottom * self.bottom >= 0.0
    }

    /// The transmittance table's (u, v) of (r, mu) (Bruneton 2017's mapping; a ray to the top that does not meet the ground).
    pub fn transmittance_uv(&self, r: f64, mu: f64) -> (f64, f64) {
        let (w, h) = TRANSMITTANCE_SIZE;
        let big_h = (self.top * self.top - self.bottom * self.bottom).sqrt();
        let rho = (r * r - self.bottom * self.bottom).max(0.0).sqrt();
        let d = self.distance_to_top(r, mu);
        let d_min = self.top - r;
        let d_max = rho + big_h;
        let x_mu = if d_max > d_min {
            (d - d_min) / (d_max - d_min)
        } else {
            0.0
        };
        (to_unit(x_mu, w), to_unit(rho / big_h, h))
    }

    /// The (r, mu) of the transmittance table's (u, v).
    pub fn transmittance_r_mu(&self, u: f64, v: f64) -> (f64, f64) {
        let (w, h) = TRANSMITTANCE_SIZE;
        let x_mu = from_unit(u, w);
        let x_r = from_unit(v, h);
        let big_h = (self.top * self.top - self.bottom * self.bottom).sqrt();
        let rho = big_h * x_r;
        let r = (rho * rho + self.bottom * self.bottom).sqrt();
        let d_min = self.top - r;
        let d_max = rho + big_h;
        let d = d_min + x_mu * (d_max - d_min);
        let mu = if d == 0.0 {
            1.0
        } else {
            ((big_h * big_h - rho * rho - d * d) / (2.0 * r * d)).clamp(-1.0, 1.0)
        };
        (r, mu)
    }

    /// The transmittance from radius `r` along `mu` to the top, by direct integration in `steps` midpoint steps.
    pub fn transmittance_direct(&self, r: f64, mu: f64, steps: usize) -> Rgb {
        let d = self.distance_to_top(r, mu);
        let dt = d / steps as f64;
        let mut depth = DVec3::ZERO;
        for i in 0..steps {
            let t = (i as f64 + 0.5) * dt;
            let h = (r * r + t * t + 2.0 * r * mu * t).sqrt() - self.bottom;
            depth += DVec3::from(self.medium(h).extinction) * dt;
        }
        depth.map(|v| (-v).exp()).to_array()
    }
}

/// Rayleigh's phase function.
pub fn rayleigh_phase(nu: f64) -> f64 {
    3.0 / (16.0 * std::f64::consts::PI) * (1.0 + nu * nu)
}

/// Cornette and Shanks' phase function (Mie, asymmetry `g`).
pub fn mie_phase(nu: f64, g: f64) -> f64 {
    let g2 = g * g;
    3.0 / (8.0 * std::f64::consts::PI) * (1.0 - g2) * (1.0 + nu * nu)
        / ((2.0 + g2) * (1.0 + g2 - 2.0 * g * nu).max(1e-12).powf(1.5))
}

/// The horizon's elevation (radians, below 0) seen from radius `r`.
pub fn horizon_elevation(a: &Atmosphere, r: f64) -> f64 {
    (a.bottom / r).clamp(-1.0, 1.0).asin() - std::f64::consts::FRAC_PI_2
}

/// The sky view table's (azimuth from the sun 0..π, elevation) of its (u, v), for a camera at radius `r`.
pub fn sky_view_angles(a: &Atmosphere, r: f64, u: f64, v: f64) -> (f64, f64) {
    let (w, h) = SKY_VIEW_SIZE;
    let az = from_unit(u, w).clamp(0.0, 1.0) * std::f64::consts::PI;
    let vx = from_unit(v, h).clamp(0.0, 1.0);
    let eh = horizon_elevation(a, r);
    let half = std::f64::consts::FRAC_PI_2;
    let el = if vx < 0.5 {
        let c = 1.0 - 2.0 * vx;
        eh + (half - eh) * c * c
    } else {
        let c = 2.0 * vx - 1.0;
        eh - (half + eh) * c * c
    };
    (az, el)
}

/// [`sky_view_angles`]'s inverse: the (u, v) of an azimuth from the sun and an elevation.
pub fn sky_view_uv(a: &Atmosphere, r: f64, az: f64, el: f64) -> (f64, f64) {
    let (w, h) = SKY_VIEW_SIZE;
    let eh = horizon_elevation(a, r);
    let half = std::f64::consts::FRAC_PI_2;
    let vx = if el >= eh {
        let c = ((el - eh) / (half - eh)).clamp(0.0, 1.0).sqrt();
        0.5 * (1.0 - c)
    } else {
        let c = ((eh - el) / (half + eh)).clamp(0.0, 1.0).sqrt();
        0.5 * (1.0 + c)
    };
    (
        to_unit((az / std::f64::consts::PI).clamp(0.0, 1.0), w),
        to_unit(vx, h),
    )
}

/// The world direction of an azimuth from the sun and an elevation (the sun toward `sun`, z up).
pub fn direction(sun: [f64; 3], az: f64, el: f64) -> [f64; 3] {
    let h = (sun[0] * sun[0] + sun[1] * sun[1]).sqrt();
    let (sx, sy) = if h > 1e-9 {
        (sun[0] / h, sun[1] / h)
    } else {
        (1.0, 0.0)
    };
    let (ca, sa) = (az.cos(), az.sin());
    let (ce, se) = (el.cos(), el.sin());
    // The sun's horizontal axis and the one at a right angle to it (anticlockwise).
    [ce * (ca * sx - sa * sy), ce * (ca * sy + sa * sx), se]
}

/// The f64 CPU integrator: its own transmittance table (for the inner sun rays of the multiple scattering only) and multiple
/// scattering table, both finer than the GPU's.
pub struct Reference {
    pub atmosphere: Atmosphere,
    t_table: Vec<DVec3>,
    ms_table: Vec<DVec3>,
}

impl Reference {
    pub fn new(atmosphere: Atmosphere) -> Reference {
        let a = atmosphere;
        let (tw, th) = REF_T_TABLE;
        let mut t_table = Vec::with_capacity(tw * th);
        for j in 0..th {
            for i in 0..tw {
                let (r, mu) = ref_t_r_mu(
                    &a,
                    (i as f64 + 0.5) / tw as f64,
                    (j as f64 + 0.5) / th as f64,
                );
                t_table.push(DVec3::from(a.transmittance_direct(r, mu, 256)));
            }
        }
        let mut reference = Reference {
            atmosphere,
            t_table,
            ms_table: Vec::new(),
        };
        // The multiple scattering table, its rows on worker threads.
        let (nu, nh) = REF_MS_SIZE;
        let workers = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .clamp(1, 16);
        let per = nh.div_ceil(workers);
        let rows: Vec<Vec<DVec3>> = {
            let reference = &reference;
            crate::par::each_worker(workers, |w| {
                let mut out = Vec::new();
                for j in w * per..((w + 1) * per).min(nh) {
                    let hv = (j as f64 + 0.5) / nh as f64;
                    let r = a.bottom + (a.top - a.bottom) * hv * hv;
                    for i in 0..nu {
                        let mu_s = 2.0 * (i as f64 + 0.5) / nu as f64 - 1.0;
                        out.push(reference.multiscatter_at(r, mu_s));
                    }
                }
                out
            })
        };
        reference.ms_table = rows.into_iter().flatten().collect();
        reference
    }

    /// The transmittance toward the sun from (r, mu): 0 where the planet hides it, else the fine table's (bilinear).
    fn sun_transmittance_table(&self, r: f64, mu: f64) -> DVec3 {
        let a = &self.atmosphere;
        if a.hits_ground(r, mu) {
            return DVec3::ZERO;
        }
        let (u, v) = ref_t_uv(a, r, mu);
        bilinear(&self.t_table, REF_T_TABLE.0, REF_T_TABLE.1, u, v)
    }

    /// The transmittance toward the sun from (r, mu) by direct integration.
    pub fn sun_transmittance(&self, r: f64, mu: f64) -> Rgb {
        let a = &self.atmosphere;
        if a.hits_ground(r, mu) {
            return [0.0; 3];
        }
        a.transmittance_direct(r, mu, REF_TRANSMITTANCE_STEPS)
    }

    /// Hillaire's Ψms at (r, μsun) by direct integration over the sphere (spherical Fibonacci directions).
    fn multiscatter_at(&self, r: f64, mu_s: f64) -> DVec3 {
        let sun = DVec3::new((1.0 - mu_s * mu_s).max(0.0).sqrt(), 0.0, mu_s);
        let n = REF_MS_DIRECTIONS;
        let mut l2 = DVec3::ZERO;
        let mut fms = DVec3::ZERO;
        let golden = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
        for k in 0..n {
            let z = 1.0 - 2.0 * (k as f64 + 0.5) / n as f64;
            let s = (1.0 - z * z).max(0.0).sqrt();
            let phi = golden * k as f64;
            let w = DVec3::new(s * phi.cos(), s * phi.sin(), z);
            let (l, f) = self.ms_ray(r, w, sun);
            l2 += l;
            fms += f;
        }
        let l2 = l2 * (1.0 / n as f64);
        let fms = fms * (1.0 / n as f64);
        l2 / (1.0 - fms).max(DVec3::splat(1e-6))
    }

    /// One direction of the multiple scattering's integral: the second order's light (isotropic phase, the ground's albedo) and fms.
    fn ms_ray(&self, r: f64, w: DVec3, sun: DVec3) -> (DVec3, DVec3) {
        let a = &self.atmosphere;
        let mu = w[2];
        let ground = a.hits_ground(r, mu);
        let d = if ground {
            a.distance_to_bottom(r, mu)
        } else {
            a.distance_to_top(r, mu)
        };
        let iso = 1.0 / (4.0 * std::f64::consts::PI);
        let steps = REF_MS_STEPS;
        let dt = d / steps as f64;
        let mut t_path = DVec3::ONE;
        let mut l = DVec3::ZERO;
        let mut f = DVec3::ZERO;
        for i in 0..steps {
            let t = (i as f64 + 0.5) * dt;
            let p = DVec3::new(w[0] * t, w[1] * t, r + w[2] * t);
            let rx = p.length();
            let mu_s = p.dot(sun) / rx;
            let m = a.medium(rx - a.bottom);
            let extinction = DVec3::from(m.extinction);
            let sigma_s = DVec3::from(m.rayleigh) + m.mie;
            let tsun = self.sun_transmittance_table(rx, mu_s);
            let s = sigma_s * tsun * iso;
            let step_t = extinction.map(|e| (-e * dt).exp());
            let integ = |src: DVec3| (src - src * step_t) / extinction.max(DVec3::splat(1e-12));
            l += t_path * integ(s);
            f += t_path * integ(sigma_s);
            t_path *= step_t;
        }
        if ground {
            let p = DVec3::new(w[0] * d, w[1] * d, r + w[2] * d);
            let n = p / p.length().max(1e-300);
            let rx = a.bottom;
            let mu_s = n.dot(sun);
            let tsun = self.sun_transmittance_table(rx, mu_s);
            let lit = mu_s.clamp(0.0, 1.0) / std::f64::consts::PI;
            l += t_path * tsun * DVec3::from(a.ground_albedo) * lit;
        }
        (l, f)
    }

    /// Ψms at (r, μsun) from the fine table (bilinear).
    pub fn multiscatter(&self, r: f64, mu_s: f64) -> Rgb {
        let a = &self.atmosphere;
        let (nu, nh) = REF_MS_SIZE;
        let u = (mu_s * 0.5 + 0.5).clamp(0.0, 1.0);
        let v = ((r - a.bottom) / (a.top - a.bottom)).clamp(0.0, 1.0).sqrt();
        bilinear(&self.ms_table, nu, nh, u, v).to_array()
    }

    /// The sky's light toward the camera at radius `r` from the direction `view` with the sun toward `sun` (both unit, z up),
    /// for a sun of 1: the single scattering with the sun's transmittance by direct integration, plus the multiple scattering.
    pub fn sky(&self, r: f64, view: [f64; 3], sun: [f64; 3]) -> Rgb {
        let (view, sun) = (DVec3::from(view), DVec3::from(sun));
        let a = &self.atmosphere;
        let mu = view[2];
        let d = if a.hits_ground(r, mu) {
            a.distance_to_bottom(r, mu)
        } else {
            a.distance_to_top(r, mu)
        };
        let nu = view.dot(sun);
        let pr = rayleigh_phase(nu);
        let pm = mie_phase(nu, a.mie_g);
        let steps = REF_VIEW_STEPS;
        let dt = d / steps as f64;
        let mut t_path = DVec3::ONE;
        let mut l = DVec3::ZERO;
        for i in 0..steps {
            let t = (i as f64 + 0.5) * dt;
            let p = DVec3::new(view[0] * t, view[1] * t, r + view[2] * t);
            let rx = p.length();
            let mu_s = p.dot(sun) / rx;
            let m = a.medium(rx - a.bottom);
            let (rayleigh, extinction) = (DVec3::from(m.rayleigh), DVec3::from(m.extinction));
            let tsun = DVec3::from(self.sun_transmittance(rx, mu_s));
            let psi = DVec3::from(self.multiscatter(rx, mu_s));
            let s = (rayleigh * pr + m.mie * pm) * tsun + (rayleigh + m.mie) * psi;
            let step_t = extinction.map(|e| (-e * dt).exp());
            let integ = (s - s * step_t) / extinction.max(DVec3::splat(1e-12));
            l += t_path * integ;
            t_path *= step_t;
        }
        l.to_array()
    }
}

/// The CPU's own transmittance table mapping (Bruneton's, at the reference table's size).
fn ref_t_uv(a: &Atmosphere, r: f64, mu: f64) -> (f64, f64) {
    let big_h = (a.top * a.top - a.bottom * a.bottom).sqrt();
    let rho = (r * r - a.bottom * a.bottom).max(0.0).sqrt();
    let d = a.distance_to_top(r, mu);
    let d_min = a.top - r;
    let d_max = rho + big_h;
    let x_mu = if d_max > d_min {
        (d - d_min) / (d_max - d_min)
    } else {
        0.0
    };
    (x_mu.clamp(0.0, 1.0), (rho / big_h).clamp(0.0, 1.0))
}

fn ref_t_r_mu(a: &Atmosphere, x_mu: f64, x_r: f64) -> (f64, f64) {
    let big_h = (a.top * a.top - a.bottom * a.bottom).sqrt();
    let rho = big_h * x_r;
    let r = (rho * rho + a.bottom * a.bottom).sqrt();
    let d_min = a.top - r;
    let d_max = rho + big_h;
    let d = d_min + x_mu * (d_max - d_min);
    let mu = if d == 0.0 {
        1.0
    } else {
        ((big_h * big_h - rho * rho - d * d) / (2.0 * r * d)).clamp(-1.0, 1.0)
    };
    (r, mu)
}

/// A table of `w` x `h` cell-centred values sampled at (u, v) in [0, 1] (bilinear, clamped).
fn bilinear(t: &[DVec3], w: usize, h: usize, u: f64, v: f64) -> DVec3 {
    let x = (u * w as f64 - 0.5).clamp(0.0, (w - 1) as f64);
    let y = (v * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let at = |i: usize, j: usize| t[j * w + i];
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// The atmosphere as the sky uniform's floats (`sky.wgsl`'s `Atmosphere`): radii, then Rayleigh, Mie and ozone, the ground.
pub fn uniform_floats(a: &Atmosphere) -> [f32; 20] {
    let f = |v: f64| v as f32;
    [
        f(a.bottom),
        f(a.top),
        f(a.bottom + CAMERA_HEIGHT),
        0.0,
        f(a.rayleigh[0]),
        f(a.rayleigh[1]),
        f(a.rayleigh[2]),
        f(a.rayleigh_height),
        f(a.mie_scattering),
        f(a.mie_absorption),
        f(a.mie_height),
        f(a.mie_g),
        f(a.ozone[0]),
        f(a.ozone[1]),
        f(a.ozone[2]),
        f(a.ozone_centre),
        f(a.ozone_half_width),
        f(a.ground_albedo[0]),
        f(a.ground_albedo[1]),
        f(a.ground_albedo[2]),
    ]
}

/// The sky view table's texels a check compares: a 16 x 16 grid of texel centres over the whole table (256 directions).
pub fn gate_texels() -> Vec<(u32, u32)> {
    let (w, h) = SKY_VIEW_SIZE;
    let mut out = Vec::with_capacity(256);
    for j in 0..16u32 {
        for i in 0..16u32 {
            let x = ((f64::from(i) + 0.5) * f64::from(w) / 16.0 - 0.5).round() as u32;
            let y = ((f64::from(j) + 0.5) * f64::from(h) / 16.0 - 0.5).round() as u32;
            out.push((x.min(w - 1), y.min(h - 1)));
        }
    }
    out
}

#[cfg(test)]
#[path = "atmosphere_tests.rs"]
mod tests;
