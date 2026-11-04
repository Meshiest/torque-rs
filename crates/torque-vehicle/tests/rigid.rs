//! `Rigid`: hand-worked chains.

use torque_vehicle::Rigid;

/// A body at rest at the origin, unit inertia, mass 2: one integrate of 0.032 s under a force of (0, 0, -40) (gravity on 2 kg)
/// and a torque about z. Worked by hand: AM.z = f32(f32(1*0.032) + 0) = 0.032; P unchanged (LV was 0); LM.z =
/// f32(f32(-40*0.032) + 0) = -1.28; LV.z = f32(-1.28 * 0.5) = -0.64; IW = identity (Q identity), so AV.z = 0.032; the world
/// centre of mass is P.
#[test]
fn integrate_chain_by_hand() {
    let mut r = Rigid {
        mass: 2.0,
        one_over_mass: 0.5,
        force: [0.0, 0.0, -40.0],
        torque: [0.0, 0.0, 1.0],
        ..Rigid::default()
    };
    r.integrate(0.032);
    assert_eq!(r.ang_mom, [0.0, 0.0, 0.032]);
    assert_eq!(
        r.lin_mom,
        [0.0, 0.0, (-40.0f64 * f64::from(0.032f32)) as f32]
    );
    assert_eq!(r.lin_vel[2], (f64::from(r.lin_mom[2]) * 0.5) as f32);
    assert_eq!(r.ang_vel, [0.0, 0.0, 0.032]);
    assert_eq!(r.lin_pos, [0.0; 3]);
    // A second step turns the body about z by 0.032 rad/s * 0.032 s: the quaternion's z is sin(-0.5 * angle * dt) * ... (the
    // half angle negated), its w the cosine.
    r.force = [0.0; 3];
    r.torque = [0.0; 3];
    r.integrate(0.032);
    let s = (0.032f32 * 0.032 * -0.5).sin();
    let dz = s * (1.0 / 0.032) * 0.032;
    assert!(r.ang_pos[2] != 0.0 && (r.ang_pos[2] - dz).abs() < 1e-7);
    assert_eq!(r.lin_vel[2], r.lin_mom[2] * 0.5);
}

/// `updateVelocity` takes the velocity from the momentum as it is, however fast: a momentum of 300 per unit mass along x is a
/// speed of 300.
#[test]
fn velocities_follow_the_momenta_unclamped() {
    let mut r = Rigid {
        lin_mom: [300.0, 0.0, 0.0],
        ..Rigid::default()
    };
    r.update_velocity();
    assert_eq!(r.lin_vel, [300.0, 0.0, 0.0]);
    assert_eq!(r.lin_mom, [300.0, 0.0, 0.0]);
    // A NaN momentum gives a NaN velocity on its axis only.
    let mut r = Rigid {
        lin_mom: [f32::NAN, 0.0, 0.0],
        ..Rigid::default()
    };
    r.apply_impulse([0.0; 3], [0.0; 3]);
    assert!(r.lin_vel[0].is_nan() && r.lin_vel[1] == 0.0);
}

/// `applyImpulse` at the centre of mass of a 4 kg body: the momentum gains J, the velocity J/4, no spin.
#[test]
fn impulse_chain_by_hand() {
    let mut r = Rigid {
        mass: 4.0,
        one_over_mass: 0.25,
        at_rest: true,
        ..Rigid::default()
    };
    r.apply_impulse([0.0; 3], [8.0, 0.0, -2.0]);
    assert!(!r.at_rest);
    assert_eq!(r.lin_mom, [8.0, 0.0, -2.0]);
    assert_eq!(r.lin_vel, [2.0, 0.0, -0.5]);
    assert_eq!(r.ang_mom, [0.0; 3]);
    // Off centre by +1 in y: r x J = (1*(-2) - 0*0, 0*8 - (-2)*0, 0*0 - 8*1) = (-2, 0, -8), IW identity.
    r.apply_impulse([0.0, 1.0, 0.0], [8.0, 0.0, -2.0]);
    assert_eq!(r.ang_mom, [-2.0, 0.0, -8.0]);
    assert_eq!(r.ang_vel, [-2.0, 0.0, -8.0]);
}

/// `fullInverse` of a diagonal matrix inverts each diagonal element (to a float's rounding).
#[test]
fn full_inverse_diagonal_case() {
    let mut m = torque_collide::math::IDENTITY;
    m[0] = 1.5;
    m[5] = 2.25;
    m[10] = 3.75;
    assert!(torque_vehicle::math::full_inverse(&mut m));
    for (i, v) in [(0, 1.5f32), (5, 2.25), (10, 3.75), (15, 1.0)] {
        assert!((m[i] - 1.0 / v).abs() < 1e-6, "element {i}: {}", m[i]);
    }
    assert_eq!(m[1], 0.0);
    // A determinant below 1e-5 leaves the matrix alone.
    let mut z = torque_collide::math::IDENTITY;
    z[0] = 1e-6;
    assert!(!torque_vehicle::math::full_inverse(&mut z));
    assert_eq!(z[0], 1e-6);
}

/// TGE's integrator has no floor and no speed limit: a body below z = -1 keeps falling at its own speed.
#[test]
fn integrate_has_no_floor_and_no_speed_limit() {
    let mut r = Rigid {
        lin_pos: [0.0, 0.0, -5.0],
        lin_mom: [0.0, 0.0, -500.0],
        ..Rigid::default()
    };
    r.update_velocity();
    r.integrate(0.032);
    assert_eq!(r.lin_vel[2], -500.0);
    assert!(r.lin_pos[2] < -5.0);
}
