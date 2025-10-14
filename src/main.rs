use clap::{Arg, Command};
use runner::{Error, Runner};
use std::path::PathBuf;

mod runner;

fn main() -> Result<(), Error> {
    let cmd = Command::new("hash")
        .about("Headless autorun")
        .arg_required_else_help(true)
        .args([
            Arg::new("id")
                .long("id")
                .short('i')
                .env("HASH_HOST")
                .help("Host id"),
            Arg::new("decoder")
                .long("decoder")
                .short('d')
                .env("HASH_DECODER")
                .help("Script decoder"),
            Arg::new("encoder")
                .long("encoder")
                .short('e')
                .env("HASH_ENCODER")
                .help("Stdout encoder"),
        ])
        .arg(
            Arg::new("path")
                .required(true)
                .help("Script path or directory"),
        )
        .arg(
            Arg::new("watch")
                .long("watch")
                .short('w')
                .num_args(0)
                .hide(cfg!(not(target_os = "linux")))
                .help("Watch for removable media"),
        )
        .get_matches();

    let path = cmd
        .get_one::<String>("path")
        .map(PathBuf::from)
        .expect("required");
    let host_id = cmd
        .get_one::<String>("id")
        .map(String::from)
        .unwrap_or(format!("Hash host v{}", env!("CARGO_PKG_VERSION")));
    let decoder = cmd.get_one::<String>("decoder").map(String::from);
    let encoder = cmd.get_one::<String>("encoder").map(String::from);
    let runner = Runner::new(host_id, decoder, encoder);

    #[cfg(target_os = "linux")]
    {
        runner.start(&path, cmd.get_flag("watch"))
    }

    #[cfg(not(target_os = "linux"))]
    {
        runner.start(&path)
    }
}
