//! The command line: flags and the values after them (`--port 28000`, `--name Ann`, `--shots <dir>`), each missing or bad
//! value an error that names its flag.
//!
//! ```
//! use torque_app_server::cli::Args;
//!
//! let mut args = Args::new(["--port", "28001", "--name"].map(String::from));
//! assert_eq!(args.next().as_deref(), Some("--port"));
//! assert_eq!(args.number::<u16>("--port"), Ok(28001));
//! assert_eq!(args.next().as_deref(), Some("--name"));
//! assert_eq!(args.value("--name", "a name"), Err("--name needs a name".to_string()));
//! ```

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::str::FromStr;

/// The arguments left to read.
#[derive(Debug, Clone)]
pub struct Args {
    rest: std::vec::IntoIter<String>,
}

impl Args {
    pub fn new(args: impl IntoIterator<Item = String>) -> Args {
        Args {
            rest: args.into_iter().collect::<Vec<_>>().into_iter(),
        }
    }

    /// The process's arguments after its name.
    pub fn from_env() -> Args {
        Args::new(std::env::args().skip(1))
    }

    /// The value after `flag`, or why there is none: `<flag> needs <what>`.
    pub fn value(&mut self, flag: &str, what: &str) -> Result<String, String> {
        self.rest
            .next()
            .ok_or_else(|| format!("{flag} needs {what}"))
    }

    /// The folder after `flag`.
    pub fn dir(&mut self, flag: &str) -> Result<PathBuf, String> {
        self.value(flag, "a folder").map(PathBuf::from)
    }

    /// The number after `flag`.
    pub fn number<T: FromStr>(&mut self, flag: &str) -> Result<T, String> {
        let text = self.value(flag, "a value")?;
        text.parse().map_err(|_| format!("{flag} needs a number"))
    }

    /// The IPv4 address after `flag`.
    pub fn ipv4(&mut self, flag: &str) -> Result<Ipv4Addr, String> {
        let text = self.value(flag, "an address")?;
        text.parse()
            .map_err(|_| format!("{flag} needs an IPv4 address, not {text:?}"))
    }
}

impl Iterator for Args {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        self.rest.next()
    }
}

/// The error for an argument no flag matches.
pub fn unknown(arg: &str) -> String {
    format!("unknown argument {arg}")
}
