//! The area lights' shading (`area.wgsl`) on the GPU against references summed over the lights' own surfaces: at 8 points on
//! planes about a ceiling panel and a window (rectangles) and about a ceiling tube, the irradiance the shader's form factor gives
//! is within 3 % of a 64 x 64 sample sum over the shape (a rectangle's face; a tube's side, and 16 x 16 over each of its ends;
//! each sample where the point sees it), the planes' horizons cutting some of the shapes. The specular (the quad turned by the fitted table's inverse matrix) follows
//! the GGX lobe's sum over the shape within the fit's error.
//!
//! The probe is a compute pass of the module the forward pass takes (`area.wgsl` after common.wgsl's PI) with one entry point:
//! each case's irradiance per unit of the light's color, its specular share and the quad's radiance.

use glam::{DVec3, dvec3};
use torque_render_core::{Gpu, wgpu};
use torque_render_hd::shaders::FILES;
use torque_render_scene::hd::ltc;
use wgpu::util::DeviceExt;

/// The reference's samples a side over a shape (a rectangle's face, a tube's side), and over each end of a tube.
const SAMPLES: usize = 64;
const END_SAMPLES: usize = 16;
const TOLERANCE: f64 = 0.03;

#[derive(Clone, Copy, Debug)]
enum Shape {
    /// Center, the two half sides (lit toward u x v).
    Rect(DVec3, DVec3, DVec3),
    /// Center, the half axis, the radius.
    Tube(DVec3, DVec3, f64),
}

/// A point on a plane lit by `shape`: where, the plane's normal and the eye's direction from it.
#[derive(Clone, Copy, Debug)]
struct Case {
    shape: Shape,
    p: DVec3,
    n: DVec3,
    v: DVec3,
    /// The lobe's roughness (the specular cases) and the table's cell there.
    rough: f64,
}

const PROBE: &str = "
struct Probe {
    p: vec4<f32>,
    n: vec4<f32>,
    v: vec4<f32>,
    // xyz the center, w the kind (1 a rectangle, 2 a tube).
    center: vec4<f32>,
    a: vec4<f32>,
    b: vec4<f32>,
    minv: vec4<f32>,
};
@group(0) @binding(0) var<storage, read> probes: array<Probe>;
@group(0) @binding(1) var<storage, read_write> results: array<vec4<f32>>;

@compute @workgroup_size(1)
fn probe(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = probes[id.x];
    let m = area_frame(c.n.xyz, c.v.xyz);
    let shares = area_shares(vec4<f32>(c.a.xyz, c.center.w), c.b, c.center.xyz, c.p.xyz, m, area_minv(c.minv) * m);
    results[id.x] = vec4<f32>(PI * shares.z * shares.x, shares.y, shares.z, 0.0);
}
";

fn file(name: &str) -> &'static str {
    FILES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
        .expect("the shader file")
}

/// Each case's (irradiance per unit color, specular form factor, radiance per unit color) from the GPU, or `None` without one.
fn probe(cases: &[Case]) -> Option<Vec<[f32; 3]>> {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return None;
        }
    };
    let pi = file("common.wgsl")
        .lines()
        .find(|l| l.starts_with("const PI: f32"))
        .expect("PI");
    let source = format!("{pi}\n{}\n{PROBE}", file("area.wgsl"));
    let device = gpu.device();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("area light probe"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("area light probe"),
        layout: None,
        module: &module,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let f = |v: DVec3, w: f64| [v.x as f32, v.y as f32, v.z as f32, w as f32];
    let mut words: Vec<f32> = Vec::new();
    for c in cases {
        let (center, a, b) = match c.shape {
            Shape::Rect(center, u, v) => (f(center, 1.0), f(u, 0.0), f(v, 0.0)),
            Shape::Tube(center, t, r) => (f(center, 2.0), f(t, 0.0), [0.0, 0.0, 0.0, r as f32]),
        };
        let cell = ltc::lookup(c.rough as f32, c.n.dot(c.v).max(1e-4) as f32);
        for w in [
            f(c.p, 1.0),
            f(c.n, 0.0),
            f(c.v, 0.0),
            center,
            a,
            b,
            cell.minv,
        ] {
            words.extend_from_slice(&w);
        }
    }
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("area light probes"),
        contents: &words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<u8>>(),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let size = (cases.len() * 16) as u64;
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("area light results"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("area light readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("area light probe"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(cases.len() as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
    gpu.queue().submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("mapped"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the probe finishes");
    let data = slice.get_mapped_range().expect("the mapped bytes");
    let v: Vec<f32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    Some(
        v.as_chunks::<4>()
            .0
            .iter()
            .map(|r| [r[0], r[1], r[2]])
            .collect(),
    )
}

/// The shape's surface samples: each point, its normal and its area.
fn surface(shape: Shape) -> Vec<(DVec3, DVec3, f64)> {
    let n = SAMPLES;
    let mid = |k: usize| (k as f64 + 0.5) / n as f64;
    match shape {
        Shape::Rect(c, u, v) => {
            let normal = u.cross(v).normalize();
            let area = 4.0 * u.cross(v).length() / (n * n) as f64;
            (0..n * n)
                .map(|k| {
                    let (s, t) = (2.0 * mid(k % n) - 1.0, 2.0 * mid(k / n) - 1.0);
                    (c + u * s + v * t, normal, area)
                })
                .collect()
        }
        Shape::Tube(c, t, r) => {
            let axis = t.normalize();
            let e1 = axis.any_orthonormal_vector();
            let e2 = axis.cross(e1);
            let area = 2.0 * t.length() * 2.0 * std::f64::consts::PI * r / (n * n) as f64;
            let side = (0..n * n).map(|k| {
                let s = 2.0 * mid(k % n) - 1.0;
                let phi = 2.0 * std::f64::consts::PI * mid(k / n);
                let normal = e1 * phi.cos() + e2 * phi.sin();
                (c + t * s + normal * r, normal, area)
            });
            // Each end's disk in rings of equal area.
            let m = END_SAMPLES;
            let ring = |k: usize| (k as f64 + 0.5) / m as f64;
            let end_area = std::f64::consts::PI * r * r / (m * m) as f64;
            let ends = [1.0, -1.0].into_iter().flat_map(move |sign| {
                (0..m * m).map(move |k| {
                    let rho = r * ring(k % m).sqrt();
                    let phi = 2.0 * std::f64::consts::PI * ring(k / m);
                    let at = c + t * sign + (e1 * phi.cos() + e2 * phi.sin()) * rho;
                    (at, axis * sign, end_area)
                })
            });
            side.chain(ends).collect()
        }
    }
}

/// The light's radiance per unit of its color (the color over the area it shows straight in front of it).
fn radiance(shape: Shape) -> f64 {
    match shape {
        Shape::Rect(_, u, v) => 1.0 / u.cross(v).length(),
        Shape::Tube(_, t, r) => 1.0 / (t.length() * r),
    }
}

/// The sum over the shape's samples the point sees of `f(l) cos_r cos_l / d^2 dA` (`l` toward the sample).
fn sum_over(case: &Case, f: impl Fn(DVec3) -> f64) -> f64 {
    surface(case.shape)
        .into_iter()
        .map(|(x, normal, area)| {
            let to = x - case.p;
            let d2 = to.length_squared();
            let l = to / d2.sqrt();
            let cos_l = -normal.dot(l);
            let cos_r = case.n.dot(l);
            if cos_l <= 0.0 || cos_r <= 0.0 {
                return 0.0;
            }
            f(l) * cos_r * cos_l / d2 * area
        })
        .sum()
}

/// GGX times its height-correlated visibility (F = 1, no cosine), as `brdf.wgsl`'s `specular` over pi.
fn ggx(n: DVec3, v: DVec3, l: DVec3, rough: f64) -> f64 {
    let h = (v + l).normalize();
    let a = ltc::alpha_of(rough);
    let a2 = a * a;
    let (nh, nv, nl) = (n.dot(h).max(0.0), n.dot(v).max(1e-4), n.dot(l).max(0.0));
    let d = nh * nh * (a2 - 1.0) + 1.0;
    let dist = a2 / (std::f64::consts::PI * d * d);
    let gv = nl * (nv * nv * (1.0 - a2) + a2).sqrt();
    let gl = nv * (nl * nl * (1.0 - a2) + a2).sqrt();
    dist * 0.5 / (gv + gl).max(1e-12)
}

fn case(shape: Shape, p: DVec3, n: DVec3) -> Case {
    let n = n.normalize();
    Case {
        shape,
        p,
        n,
        v: (n + dvec3(0.3, -0.2, 0.1)).normalize(),
        rough: 0.5,
    }
}

/// Each case's irradiance against its reference (within [`TOLERANCE`]), and the radiance the shader takes.
fn check_irradiance(cases: &[Case]) {
    let Some(got) = probe(cases) else {
        return;
    };
    let mut worst = 0.0f64;
    for (c, g) in cases.iter().zip(&got) {
        let want = radiance(c.shape) * sum_over(c, |_| 1.0);
        assert!(want > 1e-4, "{c:?}: the case is unlit ({want})");
        let off = (f64::from(g[0]) - want).abs() / want;
        worst = worst.max(off);
        assert!(
            off <= TOLERANCE,
            "{c:?}: {} vs the reference {want} ({:.2} %)",
            g[0],
            off * 100.0
        );
        assert!(
            (f64::from(g[2]) - radiance(c.shape)).abs() <= 1e-4 * radiance(c.shape),
            "{c:?}"
        );
    }
    eprintln!("worst {:.3} % of {} cases", worst * 100.0, cases.len());
}

fn rect_cases() -> Vec<Case> {
    let up = DVec3::Z;
    // A ceiling panel 1.2 x 0.6 at 2.5 lit downward, and a window 1.6 x 1.2 in the wall y = 3 lit into the room.
    let panel = Shape::Rect(
        dvec3(0.0, 0.0, 2.5),
        dvec3(0.0, 0.3, 0.0),
        dvec3(0.6, 0.0, 0.0),
    );
    let window = Shape::Rect(
        dvec3(0.0, 3.0, 1.2),
        dvec3(0.8, 0.0, 0.0),
        dvec3(0.0, 0.0, 0.6),
    );
    vec![
        case(panel, dvec3(0.0, 0.0, 0.0), up),
        case(panel, dvec3(1.5, 1.0, 0.0), up),
        case(panel, dvec3(0.3, -0.2, 1.0), dvec3(0.6, 0.0, 0.8)),
        case(panel, dvec3(0.1, 0.1, 2.0), up),
        case(window, dvec3(0.0, 1.0, 1.2), DVec3::Y),
        case(window, dvec3(1.0, 2.0, 0.0), up),
        // A table at the window's middle: its horizon cuts the window in half.
        case(window, dvec3(0.5, 2.0, 1.2), up),
        // A wall beside the window, seen at a grazing angle and cut by the wall's horizon.
        case(window, dvec3(-1.2, 2.5, 0.8), DVec3::X),
    ]
}

fn tube_cases() -> Vec<Case> {
    let up = DVec3::Z;
    // A ceiling tube 1.2 long, 8 cm across, along x at 2.5.
    let tube = Shape::Tube(dvec3(0.0, 0.0, 2.5), dvec3(0.6, 0.0, 0.0), 0.04);
    vec![
        case(tube, dvec3(0.0, 0.0, 0.0), up),
        case(tube, dvec3(1.0, 0.5, 0.0), up),
        case(tube, dvec3(2.0, -1.0, 0.0), up),
        // Past its end.
        case(tube, dvec3(0.9, 0.0, 1.5), up),
        case(tube, dvec3(-1.5, 0.0, 2.0), DVec3::X),
        case(tube, dvec3(1.5, 1.0, 1.0), dvec3(-0.5, 0.0, 0.866)),
        // Level with the tube: the horizon cuts its strip in half.
        case(tube, dvec3(0.3, 0.8, 2.5), up),
        // A wall in line with the tube: its end alone lights it.
        case(tube, dvec3(1.5, 0.0, 2.5), -DVec3::X),
    ]
}

#[test]
fn rectangle_irradiance_is_the_sampled_reference() {
    check_irradiance(&rect_cases());
}

#[test]
fn tube_irradiance_is_the_sampled_reference() {
    check_irradiance(&tube_cases());
}

#[test]
fn specular_follows_the_lobe() {
    // Rough enough lobes for the reference's samples; the fit's own error is what is left (a few percent, more toward grazing).
    let mut cases = Vec::new();
    for (k, c) in rect_cases().into_iter().chain(tube_cases()).enumerate() {
        for rough in [0.35, 0.6, 0.9] {
            // Eyes about the mirror direction of the light, at several angles.
            let center = match c.shape {
                Shape::Rect(center, _, _) | Shape::Tube(center, _, _) => center,
            };
            let l = (center - c.p).normalize();
            let mirror = 2.0 * c.n.dot(l) * c.n - l;
            let side = c.n.cross(l).normalize_or_zero();
            let v = (mirror + side * (0.15 * (k % 3) as f64)).normalize();
            if c.n.dot(v) < 0.2 {
                continue;
            }
            cases.push(Case { v, rough, ..c });
        }
    }
    let Some(got) = probe(&cases) else {
        return;
    };
    let mut worst = 0.0f64;
    for (c, g) in cases.iter().zip(&got) {
        let cell = ltc::lookup(c.rough as f32, c.n.dot(c.v) as f32);
        let ltc = f64::from(g[1]) * f64::from(cell.albedo);
        let want = sum_over(c, |l| ggx(c.n, c.v, l, c.rough));
        if want < 1e-3 {
            continue;
        }
        let off = (ltc - want).abs() / want;
        worst = worst.max(off);
        assert!(
            off <= 0.2,
            "{c:?}: {ltc} vs the lobe's {want} ({:.1} %)",
            off * 100.0
        );
    }
    eprintln!(
        "specular: worst {:.1} % of {} cases",
        worst * 100.0,
        cases.len()
    );
}
