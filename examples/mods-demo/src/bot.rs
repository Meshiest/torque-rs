//! What steers a client's player in the headless match: from where the client sees its player and the ball, it sets the move
//! globals as held keys would. With the ball in the half it attacks (or close by) it runs round behind the ball, then through
//! it toward the goal; with the ball in its own half it keeps goal, in front of its own goal across from the ball.
//!
//! It reads only the client's own world (its control object's position and the ball the props plugin's book holds) and the
//! team the server told it (`$Match::Attack`), and stands still outside play (`$Match::Phase`). Its player never turns: a
//! player faces the goal it attacks, so its forward is the attack's direction along y.

use torque::app::client::GameClient;
use torque::engine::Engine;
use torque::ghost_world::world::Pose;

use crate::assets::{GOAL_HALF, HALF_LENGTH, HALF_WIDTH};
use crate::ball_of;

/// How far the pitch reaches from its middle inside the walls, less a player's half width (x, y): a bot aims no farther.
const REACH: [f32; 2] = [HALF_WIDTH - 0.6, HALF_LENGTH - 0.6];
/// How far behind the ball a bot lines up before it runs through it.
const BEHIND: f32 = 1.3;
/// A ball this far across from the middle is by a side wall: a bot goes round it on the open side and plays it off the wall.
const WALL_NEAR: f32 = HALF_WIDTH - 2.5;
/// A ball this far along is by an end wall, and plays off it the same way.
const END_NEAR: f32 = HALF_LENGTH - 2.0;
/// A keeper goes for a ball this near it, when it is between the ball and its own goal.
const CLEAR: f32 = 2.5;
/// Where a keeper stands: this far out from its goal line, and at most this far across from the middle.
const KEEP_OUT: f32 = 2.5;
const KEEP_ACROSS: f32 = 2.0;

type V2 = [f32; 2];

fn sub(a: V2, b: V2) -> V2 {
    [a[0] - b[0], a[1] - b[1]]
}

fn add_scaled(a: V2, b: V2, s: f32) -> V2 {
    [a[0] + b[0] * s, a[1] + b[1] * s]
}

fn dot(a: V2, b: V2) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn length(a: V2) -> f32 {
    dot(a, a).sqrt()
}

fn unit(a: V2) -> V2 {
    let l = length(a);
    if l < 1e-4 {
        [0.0; 2]
    } else {
        [a[0] / l, a[1] / l]
    }
}

/// The way a player at `me` should run, a unit vector in the world (or none, at its spot), when it attacks the goal at
/// `attack` (1: the goal at +y, -1: at -y) and the ball is at `ball`.
#[must_use]
pub fn steer(me: V2, ball: V2, attack: f32) -> V2 {
    let goal_side = (ball[1] - me[1]) * attack > 0.3;
    let attacking = ball[1] * attack >= -0.5 || (goal_side && length(sub(ball, me)) < CLEAR);
    let target = if attacking {
        chase(me, ball, attack)
    } else {
        [
            (ball[0] * 0.4).clamp(-KEEP_ACROSS, KEEP_ACROSS),
            -attack * (HALF_LENGTH - KEEP_OUT),
        ]
    };
    let target = [
        target[0].clamp(-REACH[0], REACH[0]),
        target[1].clamp(-REACH[1], REACH[1]),
    ];
    let way = sub(target, me);
    if length(way) < 0.15 {
        [0.0; 2]
    } else {
        unit(way)
    }
}

/// Where a bot plays the ball at `ball` toward: the goal at `attack`, or off a wall the ball lies by (out from a side wall and
/// up the pitch; along the end wall it attacks to the goal mouth).
fn aim(ball: V2, attack: f32) -> V2 {
    let [x, y] = ball;
    if x.abs() > WALL_NEAR {
        [x - x.signum() * 4.0, y + attack * 4.0]
    } else if y.abs() > END_NEAR && x.abs() > GOAL_HALF {
        if y * attack > 0.0 {
            [0.0, y]
        } else {
            [x - x.signum() * 3.0, y + attack * 4.0]
        }
    } else {
        [0.0, attack * (HALF_LENGTH + 0.5)]
    }
}

/// Where a player at `me` that goes for the ball at `ball` runs to: behind it, then through it toward the goal at `attack`.
fn chase(me: V2, ball: V2, attack: f32) -> V2 {
    let to_goal = unit(sub(aim(ball, attack), ball));
    let behind = add_scaled(ball, to_goal, -BEHIND);
    let from_ball = sub(me, ball);
    let along = dot(from_ball, to_goal);
    let across = to_goal[0] * from_ball[1] - to_goal[1] * from_ball[0];
    if along < -0.5 && across.abs() < 0.55 {
        // Lined up behind it: run through the ball toward the goal.
        add_scaled(ball, to_goal, 1.0)
    } else if along > -0.4 {
        // Level with the ball or past it: go round it, not through it, on this side or, by a wall, on the open side.
        let round = [-to_goal[1], to_goal[0]];
        let side = if ball[0].abs() > WALL_NEAR && round[0].abs() > 0.3 {
            -ball[0].signum() * round[0].signum()
        } else if across < 0.0 {
            -1.0
        } else {
            1.0
        };
        add_scaled(behind, round, side * 1.4)
    } else {
        behind
    }
}

/// Where client `client` sees its own player, from the control object's last position or its ghost.
fn own_position(client: &GameClient) -> Option<V2> {
    client
        .with_world(|world| {
            let position = world.control.position.or_else(|| {
                let ghost = world.ghosts.get(&world.control.object?)?;
                match ghost.pose()? {
                    Pose::Euler { position, .. } => Some(position),
                    _ => None,
                }
            })?;
            Some([position[0], position[1]])
        })
        .flatten()
}

/// Where the client sees the ball.
fn ball_position(engine: &Engine) -> Option<V2> {
    let [x, y, _] = ball_of(engine)?.q.position();
    Some([x, y])
}

/// Sets client `client`'s move globals for this tick from what it sees (`engine` is the client's engine).
pub fn drive(client: &mut GameClient, engine: &Engine) {
    let attack: f32 = client.global("$Match::Attack").parse().unwrap_or(0.0);
    let way = if client.global("$Match::Phase") == "play" && attack != 0.0 {
        own_position(client)
            .zip(ball_position(engine))
            .map_or([0.0; 2], |(me, ball)| steer(me, ball, attack))
    } else {
        [0.0; 2]
    };
    // A player facing +y has +x on its right; one facing -y has both the other way.
    let (right, forward) = (way[0] * attack.signum(), way[1] * attack.signum());
    for (name, value) in [
        ("$mvForwardAction", forward.max(0.0)),
        ("$mvBackwardAction", (-forward).max(0.0)),
        ("$mvRightAction", right.max(0.0)),
        ("$mvLeftAction", (-right).max(0.0)),
    ] {
        client.set_global(name, &value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::steer;

    #[test]
    fn a_bot_runs_round_the_ball_then_through_it_toward_the_goal_it_attacks() {
        // Lined up behind the ball: straight at the +y goal.
        assert_eq!(steer([0.0, -2.0], [0.0, 0.0], 1.0), [0.0, 1.0]);
        // The same for the other team, the other way.
        assert_eq!(steer([0.0, 2.0], [0.0, 0.0], -1.0), [0.0, -1.0]);
        // Past the ball: it does not run back through it, it goes round on its own side.
        let way = steer([1.0, 1.0], [0.0, 0.0], 1.0);
        assert!(way[0] > 0.0 && way[1] < 0.0, "{way:?}");
        let way = steer([-1.0, 1.0], [0.0, 0.0], 1.0);
        assert!(way[0] < 0.0 && way[1] < 0.0, "{way:?}");
        // Level with the ball, beside it: back and round, behind it.
        let way = steer([2.0, 0.0], [0.0, 0.0], 1.0);
        assert!(way[0] < 0.0 && way[1] < 0.0, "{way:?}");
        // By a wall it goes round on the open side, never between the ball and the wall.
        let way = steer([-7.4, 1.0], [-7.0, 0.0], 1.0);
        assert!(way[0] > 0.0 && way[1] < 0.0, "{way:?}");
    }

    #[test]
    fn with_the_ball_in_its_own_half_a_bot_keeps_goal_until_the_ball_comes_near() {
        // Blue defends the goal at -y: it runs back in front of it, across from the ball.
        let way = steer([0.0, 0.0], [-5.0, -4.0], 1.0);
        assert!(way[1] < -0.9, "{way:?}");
        assert_eq!(steer([-2.0, -9.5], [-5.0, -4.0], 1.0), [0.0, 0.0]);
        // A ball that comes near is cleared: lined up behind it, the keeper runs at it.
        assert_eq!(steer([0.0, -9.5], [0.0, -7.5], 1.0), [0.0, 1.0]);
        // A ball past it, nearer its own goal, it does not run at: it goes back in front of its goal.
        let way = steer([0.0, -8.0], [1.0, -9.5], 1.0);
        assert!(way[1] < 0.0, "{way:?}");
    }
}
