use clap::Parser;
use std::{env, path};

mod dvc;
mod sangfor;
mod svc;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(value_enum)]
    mode: Mode,
    #[command(subcommand)]
    operation: Operation,
}

#[derive(clap::ValueEnum, Clone)]
enum Mode {
    Dvc,
    Svc,
    /// Sangfor VDI client: the hardcoded channel plugin list is patched on disk.
    Sangfor,
}

#[derive(clap::Subcommand)]
enum Operation {
    Register {
        dll: String,
        /// Sangfor mode only: hardcoded channel plugin entry to replace.
        #[arg(long)]
        victim: Option<String>,
        /// Sangfor mode only: restrict the registration to a single client
        /// binary, leaving the Authenticode signature of the others intact.
        #[arg(long)]
        only: Option<String>,
        /// Sangfor mode only: report actions without modifying any file.
        #[arg(long)]
        dry_run: bool,
    },
    Unregister {
        /// Sangfor mode only: restrict the restoration to a single client binary.
        #[arg(long)]
        only: Option<String>,
        /// Sangfor mode only: report actions without modifying any file.
        #[arg(long)]
        dry_run: bool,
    },
    /// Sangfor mode only: report the current state of the registration.
    Status,
}

fn dll_path(dll: &str) -> Result<String, String> {
    let path = path::Path::new(dll);

    if !path.exists() {
        return Err("invalid DLL path".into());
    }

    if !path.is_file() {
        return Err("given path is not a file".into());
    }

    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let curdir = env::current_dir().map_err(|e| format!("invalid current dir: {e}"))?;
        curdir.join(path)
    };

    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| "DLL path is not valid UTF-8".into())
}

fn main() {
    let args = Args::parse();

    if matches!(args.mode, Mode::Sangfor) {
        match args.operation {
            Operation::Register {
                dll,
                victim,
                only,
                dry_run,
            } => sangfor::register(&dll, victim.as_deref(), only.as_deref(), dry_run),
            Operation::Unregister { only, dry_run } => {
                sangfor::unregister(only.as_deref(), dry_run);
            }
            Operation::Status => sangfor::status(),
        }

        return;
    }

    match args.operation {
        Operation::Register { dll, .. } => {
            let path = match dll_path(&dll) {
                Ok(path) => path,
                Err(e) => {
                    eprintln!("{e}");
                    return;
                }
            };

            println!("DLL path = {path}");

            let file_name = path::Path::new(&path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();

            match args.mode {
                Mode::Dvc => dvc::register(&path),
                Mode::Svc => {
                    println!(
                        "Do not forget to put {file_name} in C:\\Program Files (x86)\\Citrix\\ICA Client\\ !!!!!!"
                    );
                    svc::register(&path, file_name);
                }
                Mode::Sangfor => unreachable!(),
            }
        }
        Operation::Unregister { .. } => match args.mode {
            Mode::Dvc => dvc::unregister(),
            Mode::Svc => svc::unregister(),
            Mode::Sangfor => unreachable!(),
        },
        Operation::Status => {
            eprintln!("status is only supported by the sangfor mode");
        }
    }
}
