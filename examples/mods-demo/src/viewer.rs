//! A client of a [`Session`] drawn offscreen, as a window would draw it ([`crate::draw`]): the pitch, the players and the
//! ball in shapes the client downloaded, and the board the client's own script keeps.

use torque::app::shots::Seat;
use torque::app::window::offscreen_gpu;
use torque::app::{App, Host, WorldView};
use torque::gui::GuiDraws;
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;

use crate::draw::{Drawing, world_view};
use crate::game::pitch_camera;
use crate::session::Session;

/// The picture's size.
pub const SIZE: (u32, u32) = (1280, 720);

/// A session whose client `watch` is drawn.
pub struct Viewer {
    pub session: Session,
    pub watch: usize,
    drawing: Drawing,
    /// The eye the picture is drawn from.
    pub eye: Eye,
    /// The time the session has not stepped yet (ms).
    owed: u32,
}

/// The default view: from the side and above, the whole pitch, Blue attacking to the right.
pub fn overview() -> Eye {
    pitch_camera(1.0)
}

impl Viewer {
    /// Draws client `watch` of `session`. Err: the client's files could not be shared with the drawing.
    pub fn new(mut session: Session, watch: usize) -> Result<Viewer, String> {
        let drawing = Drawing::new(&mut session.clients[watch])?;
        Ok(Viewer {
            session,
            watch,
            drawing,
            eye: overview(),
            owed: 0,
        })
    }

    /// Whether the client can read the file `path` (mounted content included).
    pub fn has_file(&self, path: &str) -> bool {
        self.drawing.has_file(path)
    }
}

impl App for Viewer {
    fn host(&mut self) -> &mut Host {
        &mut self.session.clients[self.watch].host
    }

    fn host_ref(&self) -> &Host {
        &self.session.clients[self.watch].host
    }

    fn title(&self) -> String {
        "torque-example-mods-demo".to_string()
    }

    fn log_name(&self) -> &str {
        "example-mods-demo"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        world_view(gpu)
    }

    fn advance(&mut self, ms: u32) {
        self.owed += ms;
        while self.owed >= crate::session::TICK_MS {
            self.owed -= crate::session::TICK_MS;
            self.session.step();
        }
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let client = &mut self.session.clients[self.watch];
        (self.drawing.frame(client, view, 0.016, &self.eye), self.eye)
    }
}

/// A seat that draws client `watch` of `session` at [`SIZE`], and the GPU it draws on. Err: no GPU (the test says so and
/// skips).
pub fn seat(session: Session, watch: usize) -> Result<(Seat<Viewer>, Gpu), String> {
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let seat = Seat::new(Viewer::new(session, watch)?, &gpu, SIZE)?;
    Ok((seat, gpu))
}

/// A picture of what the seat shows now.
pub fn picture(seat: &mut Seat<Viewer>, gpu: &Gpu) -> Result<Frame, String> {
    seat.picture(gpu)
}

/// Where the watched client sees the ball, and its velocity.
fn ball(seat: &Seat<Viewer>) -> Option<([f32; 3], [f32; 3])> {
    let ball = seat.game.session.ball(seat.game.watch)?;
    Some((ball.q.position(), ball.q.state().linvel))
}

/// A shot on goal: the ball in a goal's third and running at it.
fn shot_on_goal(seat: &Seat<Viewer>) -> bool {
    let playing = seat.game.session.server_global("$Soccer::Phase") == "play";
    playing
        && ball(seat).is_some_and(|([x, y, _], [_, vy, _])| {
            y.abs() > 6.5 && x.abs() < 5.0 && vy * y.signum() > 4.0
        })
}

/// Steps the session (without drawing) until `done` holds, at most `limit` ms; whether it did.
fn wait(seat: &mut Seat<Viewer>, limit: u32, done: impl Fn(&Seat<Viewer>) -> bool) -> bool {
    let mut waited = 0;
    while !done(seat) {
        if waited >= limit {
            return false;
        }
        seat.game.advance(crate::session::TICK_MS);
        waited += crate::session::TICK_MS;
    }
    true
}

/// A view of Red's goal from the pitch, close up.
pub fn goal_close() -> Eye {
    look_at([3.2, 6.0, 2.4], [0.0, 13.0, 0.9])
}

/// The pictures of a match, as client 0 draws it, written into `dir`: the kickoff, a shot on goal, the board after a goal,
/// the end of the round, and one of the goals close up. The session must hold two clients that have joined. Returns the
/// paths.
pub fn write_shots(
    session: Session,
    dir: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, String> {
    let (mut seat, gpu) = seat(session, 0)?;
    let mut shots = torque::app::shots::Shots::new(dir)?;
    let phase = |seat: &Seat<Viewer>| seat.game.session.server_global("$Soccer::Phase");
    seat.run(&gpu, 700)?;
    shots.save(&picture(&mut seat, &gpu)?, "kickoff")?;
    if !wait(&mut seat, 90_000, shot_on_goal) {
        return Err("no shot on goal".into());
    }
    seat.run(&gpu, 150)?;
    shots.save(&picture(&mut seat, &gpu)?, "shot-on-goal")?;
    if !wait(&mut seat, 90_000, |s| phase(s) == "goal") {
        return Err("no goal".into());
    }
    seat.run(&gpu, 600)?;
    shots.save(&picture(&mut seat, &gpu)?, "goal")?;
    if !wait(&mut seat, 120_000, |s| phase(s) == "over") {
        return Err("the round did not end".into());
    }
    seat.run(&gpu, 600)?;
    shots.save(&picture(&mut seat, &gpu)?, "round-over")?;
    seat.game.eye = goal_close();
    shots.save(&picture(&mut seat, &gpu)?, "goal-close")?;
    Ok(shots.paths)
}
