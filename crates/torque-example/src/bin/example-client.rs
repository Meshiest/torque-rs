//! The example's client: connects to an example server and prints the ghosts it is sent.
//!
//! `cargo run -p torque-example --bin example-client -- [--server 127.0.0.1:28000] [--seconds 5]`

use std::time::{Duration, Instant};

use torque_example::client::ExampleClient;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = "127.0.0.1:28000".parse()?;
    let mut seconds = 5u64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or(format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--server" => server = value.parse()?,
            "--seconds" => seconds = value.parse()?,
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    let mut client = ExampleClient::connect(server)?;
    let start = Instant::now();
    let mut next_print = 0;
    while start.elapsed() < Duration::from_secs(seconds) {
        let now = start.elapsed().as_millis() as u32;
        client.tick(now);
        for note in client.take_notes() {
            println!("{note:?}");
        }
        if client.connected() && now >= next_print {
            next_print = now + 1000;
            println!(
                "{} ghosts after {} packets:",
                client.world().ghosts.len(),
                client.packets()
            );
            for ghost in client.seen() {
                println!("  {ghost}");
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    client.disconnect();
    Ok(())
}
