#![allow(clippy::print_stderr, clippy::print_stdout, clippy::string_slice)]

use std::env;
use std::error::Error;

mod generate;
mod process;
mod release;
mod rpm;
mod verify;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return Err("missing command".into());
    }

    match args.remove(0).as_str() {
        "generate" => generate::run(args),
        "release" => release::run(args),
        "verify" => verify::run(args),
        "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        command => Err(format!("unknown command: {command}").into()),
    }
}

fn print_usage() {
    eprintln!(
        "Usage:
  cargo run --locked -p xtask -- generate flatpak-sources [--check]
  cargo run --locked -p xtask -- generate i18n-template [--check] [--output PATH]
  cargo run --locked -p xtask -- generate aur-stable [--check] [--skip-srcinfo] VERSION
  cargo run --locked -p xtask -- generate rpm-srpm TAG --output PATH
  cargo run --locked -p xtask -- release prepare VERSION SUMMARY
  cargo run --locked -p xtask -- release create-tag [--base TAG] [--dry-run] [--replace] [--skip-flathub] VERSION SUMMARY
  cargo run --locked -p xtask -- release update-flathub-manifest [--manifest PATH] TAG
  cargo run --locked -p xtask -- verify icons
  cargo run --locked -p xtask -- verify package-layout ROOT [PREFIX]
  cargo run --locked -p xtask -- verify release-tag TAG"
    );
}

pub(crate) fn parse_check_flag(args: Vec<String>, usage: &str) -> Result<Option<bool>> {
    let mut check = false;
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "-h" | "--help" => {
                eprintln!("{usage}");
                return Ok(None);
            }
            _ => return Err(format!("unexpected argument: {arg}").into()),
        }
    }
    Ok(Some(check))
}

pub(crate) fn ensure_no_args(args: &[String]) -> Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("unexpected argument: {}", args[0]).into())
    }
}

pub(crate) fn print_help_if_requested(args: &[String], usage: &str) -> Result<bool> {
    match args {
        [arg] if arg == "-h" || arg == "--help" => {
            eprintln!("{usage}");
            Ok(true)
        }
        [arg, ..] if arg == "-h" || arg == "--help" => {
            Err(format!("unexpected argument after {arg}: {}", args[1]).into())
        }
        _ => Ok(false),
    }
}
