use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Result;

const WAV_FILE: &str = "package-check.wav";
const WAVPACK_FILE: &str = "package-check.wv";
const MEDIA_FILES: &[&str] = &[
    "package-check.mp3",
    "package-check.flac",
    "package-check.m4a",
    "package-check.ogg",
    "package-check.opus",
    WAV_FILE,
    WAVPACK_FILE,
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
    (WAV_FILE, &["audio/x-raw,format=S16LE", "wavenc"]),
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
        gst_file(output_directory, pipeline, filename)?;
    }

    run_command(output_directory, "wavpack", wavpack_arguments())?;

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

fn gst_file(output_directory: &Path, pipeline: &[&str], filename: &str) -> Result<()> {
    run_command(
        output_directory,
        GST_LAUNCH,
        gst_arguments(pipeline, filename),
    )
}

fn gst_arguments(pipeline: &[&str], filename: &str) -> Vec<OsString> {
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
    args.push(location_argument(filename));
    args
}

fn location_argument(filename: &str) -> OsString {
    let mut argument = OsString::from("location=");
    argument.push(filename);
    argument
}

fn wavpack_arguments() -> [OsString; 5] {
    [
        OsString::from("-q"),
        OsString::from("-y"),
        OsString::from(WAV_FILE),
        OsString::from("-o"),
        OsString::from(WAVPACK_FILE),
    ]
}

fn run_command<I>(working_directory: &Path, program: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = OsString>,
{
    let status = Command::new(program)
        .current_dir(working_directory)
        .args(args)
        .status()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed with status {status}").into())
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{WAV_FILE, gst_arguments, wavpack_arguments};

    #[test]
    fn media_tools_receive_filenames_relative_to_the_output_directory() {
        let gst = gst_arguments(&["wavenc"], WAV_FILE);
        assert_has_no_directory(gst.last().expect("GStreamer output argument"));

        let wavpack = wavpack_arguments();
        assert_has_no_directory(&wavpack[2]);
        assert_has_no_directory(&wavpack[4]);
    }

    fn assert_has_no_directory(argument: &OsStr) {
        let argument = argument.to_string_lossy();
        assert!(
            !argument.contains('/'),
            "unexpected directory in {argument}"
        );
        assert!(
            !argument.contains('\\'),
            "unexpected directory in {argument}"
        );
    }
}
