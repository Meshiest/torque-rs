//! The engine protocol's optional fields of the particle datablocks, as the client holds them (the wire's block is
//! [`ghosts::datablocks::effects::ParticleExt`] and [`EmitterExt`](ghosts::datablocks::effects::EmitterExt)), and TGE's two
//! orientation flags of an emitter, which the CPU path never honored.
//!
//! A datablock a Torque-protocol server sent has none of them: every field is then its default, which leaves TGE's look and
//! motion as they are. A value read here is sanitized again ([`ParticleExt::sanitize`](ghosts::datablocks::effects::ParticleExt::sanitize)):
//! the world can hold a datablock the wire's reader did not make.

use ghost_world::tge::TgeDatablock;
use ghosts::Value;
use ghosts::codec::F32;
use ghosts::datablocks::effects::{
    EmitterExt, LIT_OFF, LIT_ON, MAX_EMITTER_BUDGET as MAX_BUDGET, ParticleExt,
};

/// Whether a particle is lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lit {
    /// The renderer's rule for the particle ([`crate::hd::particle_light::ParticleLight`]).
    #[default]
    Auto,
    On,
    Off,
}

impl Lit {
    fn of(word: u32) -> Lit {
        match word {
            LIT_ON => Lit::On,
            LIT_OFF => Lit::Off,
            _ => Lit::Auto,
        }
    }
}

/// A `ParticleData`'s fields beyond TGE's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleExtras {
    /// `turbulence`: the curl noise's acceleration (units a second squared; 0: none).
    pub turbulence: f32,
    /// `turbulenceScale`: the size of the noise's swirls (units).
    pub turbulence_scale: f32,
    /// `collide`: the particle bounces off what the depth buffer shows.
    pub collide: bool,
    /// `bounceElasticity`, `bounceFriction`.
    pub bounce_elasticity: f32,
    pub bounce_friction: f32,
    /// `lit`.
    pub lit: Lit,
    /// `softDistance`: the depth over which the sprite fades into what is behind it (units).
    pub soft_distance: f32,
    /// `flipbookBlend`: a flipbook's frame blends into the next.
    pub flipbook_blend: bool,
    /// `sizeByVelocity`: the sprite stretched along its motion, its tail this many seconds of its speed behind it.
    pub size_by_velocity: f32,
}

impl Default for ParticleExtras {
    fn default() -> ParticleExtras {
        ParticleExtras::of_ext(ParticleExt::default())
    }
}

impl ParticleExtras {
    fn of_ext(e: ParticleExt) -> ParticleExtras {
        let e = e.sanitize();
        ParticleExtras {
            turbulence: e.turbulence.get(),
            turbulence_scale: e.turbulence_scale.get(),
            collide: e.collide,
            bounce_elasticity: e.bounce_elasticity.get(),
            bounce_friction: e.bounce_friction.get(),
            lit: Lit::of(e.lit),
            soft_distance: e.soft_distance.get(),
            flipbook_blend: e.flipbook_blend,
            size_by_velocity: e.size_by_velocity.get(),
        }
    }

    /// The fields `db` holds (a field it lacks is the default).
    pub fn read(db: &TgeDatablock) -> ParticleExtras {
        let d = ParticleExt::default();
        let float = |name: &str, default: F32| match db.get(name, 0) {
            Some(Value::F32(x)) => F32::new(*x),
            _ => default,
        };
        let flag = |name: &str| matches!(db.get(name, 0), Some(Value::Bool(true)));
        ParticleExtras::of_ext(ParticleExt {
            turbulence: float("turbulence", d.turbulence),
            turbulence_scale: float("turbulenceScale", d.turbulence_scale),
            collide: flag("collide"),
            bounce_elasticity: float("bounceElasticity", d.bounce_elasticity),
            bounce_friction: float("bounceFriction", d.bounce_friction),
            lit: match db.get("lit", 0) {
                Some(Value::Int(n)) => u32::try_from(*n).unwrap_or(d.lit),
                _ => d.lit,
            },
            soft_distance: float("softDistance", d.soft_distance),
            flipbook_blend: flag("flipbookBlend"),
            size_by_velocity: float("sizeByVelocity", d.size_by_velocity),
        })
    }
}

/// How an emitter's particles are turned in the plane they are drawn in (`orientParticles`, `orientOnVelocity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orient {
    /// Billboards turned by their spin (TGE's default).
    #[default]
    Billboard = 0,
    /// A quad along the particle's emission direction, facing the eye as far as that allows (`orientParticles`).
    Emission = 1,
    /// A quad along the particle's velocity, none while it has none (`orientParticles` with `orientOnVelocity`).
    Velocity = 2,
}

impl Orient {
    /// The mode the two flags of an emitter name.
    pub fn of(orient_particles: bool, orient_on_velocity: bool) -> Orient {
        match (orient_particles, orient_on_velocity) {
            (false, _) => Orient::Billboard,
            (true, false) => Orient::Emission,
            (true, true) => Orient::Velocity,
        }
    }
}

/// A `ParticleEmitterData`'s fields beyond what [`EmitterData`](super::EmitterData) holds of TGE's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EmitterExtras {
    /// `orientParticles` and `orientOnVelocity`.
    pub orient: Orient,
    /// `trail`: drawn as a ribbon bucket.
    pub trail: bool,
    /// `budget`: the most particles the emitter may hold at once (0: no share of its own).
    pub budget: u32,
}

impl EmitterExtras {
    /// The fields `db` holds (a field it lacks is the default).
    pub fn read(db: &TgeDatablock) -> EmitterExtras {
        let flag = |name: &str| matches!(db.get(name, 0), Some(Value::Bool(true)));
        let ext = EmitterExt {
            trail: flag("trail"),
            budget: match db.get("budget", 0) {
                Some(Value::Int(n)) => (*n).clamp(0, i64::from(MAX_BUDGET)) as u32,
                _ => 0,
            },
        }
        .sanitize();
        EmitterExtras {
            orient: Orient::of(flag("orientParticles"), flag("orientOnVelocity")),
            trail: ext.trail,
            budget: ext.budget.min(MAX_BUDGET),
        }
    }
}
