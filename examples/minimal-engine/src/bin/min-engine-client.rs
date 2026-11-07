//! The example's client: connects to an example server and prints the ghosts it is sent.
//!
//! `cargo run -p torque-example-minimal-engine --bin min-engine-client -- [--server 127.0.0.1:28000] [--seconds 5]`

use std::time::{Duration, Instant};

use torque::app::cli::{Args, unknown};
use torque::app::client::ClientNet;
use torque_example_minimal_engine::{builder, client, files, ghosts};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut server, mut seconds) = ("127.0.0.1:28000".to_string(), 5);
    let mut args = Args::from_env();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server" => server = args.value(&arg, "an address")?,
            "--seconds" => seconds = args.number(&arg)?,
            _ => return Err(unknown(&arg).into()),
        }
    }
    let engine = builder(&files()?)?.build()?;
    let mut client = client(&engine, ClientNet::Loopback)?;
    client.call("connectTo", &[&server])?;
    for _ in 0..seconds {
        let second = Instant::now();
        while second.elapsed() < Duration::from_secs(1) {
            client.step(5);
            std::thread::sleep(Duration::from_millis(5));
        }
        println!("{} ghosts:", client.ghost_count());
        for (class, at) in ghosts(&client) {
            println!("  {class} at {at:?}");
        }
    }
    client.call("disconnect", &[])?;
    Ok(())
}
