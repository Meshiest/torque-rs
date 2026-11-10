//! The example's client in a window: connects to an example server and draws the ghosts it is sent.
//!
//! `cargo run -p torque-example-minimal --features window --bin example-window -- [--server 127.0.0.1:28000 | --local]`

use torque_example_minimal::window::{ExampleWindow, run};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = "127.0.0.1:28000".parse()?;
    let mut local = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--local" => local = true,
            "--server" => server = args.next().ok_or("--server needs a value")?.parse()?,
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    run(ExampleWindow::new(server, local)?)?;
    Ok(())
}
