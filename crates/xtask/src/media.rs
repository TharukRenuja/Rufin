use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Result;

const MEDIA_FILES: &[&str] = &[
    "package-check.mp3",
    "package-check.flac",
    "package-check.m4a",
    "package-check.ogg",
    "package-check.opus",
    "package-check.wav",
    "package-check.wv",
    "package-check.mka",
    "package-check.wma",
];
#[cfg(not(windows))]
const GST_LAUNCH: &str = "gst-launch-1.0";
#[cfg(windows)]
const GST_LAUNCH: &str = "gst-launch-1.0.exe";
const GST_MEDIA_FILES: &[(&str, &[&str])] = &[
    ("package-check.mp3", &["lamemp3enc"]),
    ("package-check.flac", &["flacenc"]),
    ("package-check.m4a", &["avenc_aac", "mp4mux"]),
    ("package-check.ogg", &["vorbisenc", "oggmux"]),
    ("package-check.opus", &["opusenc", "oggmux"]),
    ("package-check.wav", &["audio/x-raw,format=S16LE", "wavenc"]),
    ("package-check.mka", &["vorbisenc", "matroskamux"]),
    ("package-check.wma", &["avenc_wmav2", "asfmux"]),
];

pub(crate) fn verification_files_command(args: Vec<String>) -> Result<()> {
    let usage = "Usage: cargo run --locked -p xtask -- generate media-verification-files OUTPUT";
    if matches!(args.as_slice(), [arg] if arg == "-h" || arg == "--help") {
        eprintln!("{usage}");
        return Ok(());
    }
    if args.len() != 1 {
        return Err("generate media-verification-files requires OUTPUT".into());
    }
    generate_verification_files(&PathBuf::from(&args[0]))
}

fn generate_verification_files(output_directory: &Path) -> Result<()> {
    fs::create_dir_all(output_directory)?;
    for filename in MEDIA_FILES {
        let path = output_directory.join(filename);
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|error| format!("failed to remove {}: {error}", path.display()))?;
        }
    }

    for (filename, pipeline) in GST_MEDIA_FILES {
        gst_file(pipeline, &output_directory.join(filename))?;
    }

    let wav = output_directory.join("package-check.wav");
    let wavpack = output_directory.join("package-check.wv");
    run_command(
        "wavpack",
        [
            OsString::from("-q"),
            OsString::from("-y"),
            wav.into_os_string(),
            OsString::from("-o"),
            wavpack.into_os_string(),
        ],
    )?;

    for filename in MEDIA_FILES {
        let path = output_directory.join(filename);
        let metadata = fs::metadata(&path).map_err(|error| {
            format!(
                "missing media verification file {}: {error}",
                path.display()
            )
        })?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(format!("media verification file is empty: {}", path.display()).into());
        }
    }
    Ok(())
}

fn gst_file(pipeline: &[&str], output: &Path) -> Result<()> {
    let mut args = [
        "-q",
        "audiotestsrc",
        "num-buffers=20",
        "!",
        "audioconvert",
        "!",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    for element in pipeline {
        args.push(OsString::from(element));
        args.push(OsString::from("!"));
    }
    args.push(OsString::from("filesink"));
    args.push(location_argument(output));
    run_command(GST_LAUNCH, args)
}

fn location_argument(path: &Path) -> OsString {
    let mut argument = OsString::from("location=");
    argument.push(path.as_os_str());
    argument
}

fn run_command<I>(program: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = OsString>,
{
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed with status {status}").into())
    }
}
