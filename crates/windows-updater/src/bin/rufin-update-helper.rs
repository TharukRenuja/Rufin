#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    match windows_updater::run_helper() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}
