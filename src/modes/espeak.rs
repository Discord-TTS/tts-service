use std::sync::{LazyLock, OnceLock};

use aformat::{CapStr, ToArrayString, aformat, astr};
use itertools::Itertools as _;
use memchr::memmem::Finder;
use reqwest::header::HeaderValue;

use crate::Result;

const ESPEAK_NG_DATA_PATH: &str = "/usr/local/share/espeak-ng-data";
const MBROLA_DATA_PATH: &str = "/usr/share/mbrola/data";

struct Finders {
    replaced_with_err: Finder<'static>,
    repeat_err: Finder<'static>,
}

static MBROLA_ERR_FINDERS: LazyLock<Finders> = LazyLock::new(|| Finders {
    replaced_with_err: Finder::new(b"unknown, replaced with"),
    repeat_err: Finder::new(b"mbrowrap error: unable to get .wav header from mbrola"),
});

pub async fn get_tts(
    text: &str,
    voice: &str,
    speaking_rate: u16,
) -> Result<(bytes::Bytes, Option<HeaderValue>)> {
    if !check_voice(voice) {
        anyhow::bail!("Invalid voice: {voice}");
    }

    let voice = CapStr::<8>(voice);
    let Finders {
        repeat_err,
        replaced_with_err,
    } = &*MBROLA_ERR_FINDERS;

    // We have to loop due to random "unable to get .wav header" errors.
    let mut i = 1;
    let mut raw_wav = loop {
        let mut espeak_process = tokio::process::Command::new("espeak-ng")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .args([
                "--pho",
                "-s",
                &speaking_rate.to_arraystring(),
                "-v",
                &aformat!("mb/mb-{voice}"),
                text,
            ])
            .spawn()?;

        let espeak_stdout: std::process::Stdio = espeak_process
            .stdout
            .take()
            .expect("Failed to open espeak stdout")
            .try_into()?;

        let mbrola_process = tokio::process::Command::new("mbrola")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(espeak_stdout)
            .args([
                "-e",
                &dbg!(aformat!("{}/{voice}/{voice}", astr!(MBROLA_DATA_PATH))),
                "-",
                "-.wav",
            ])
            .spawn()?;

        let (espeak_output, mbrola_output) = tokio::try_join!(
            espeak_process.wait_with_output(),
            mbrola_process.wait_with_output(),
        )?;

        if repeat_err.find(&espeak_output.stderr).is_some() {
            i += 1;
            continue;
        }

        tracing::debug!("Generated eSpeak after {i} tries");
        if !espeak_output.stderr.is_empty() {
            let stderr_string = String::from_utf8_lossy(&espeak_output.stderr);
            tracing::error!("eSpeak Error: {stderr_string}");
        }

        if !mbrola_output.stderr.is_empty() {
            let stderr_string = String::from_utf8_lossy(&mbrola_output.stderr);
            let stderr_string = stderr_string
                .lines()
                .filter(|line| replaced_with_err.find(line.as_bytes()).is_none())
                .join("\n");

            tracing::error!("Mbrola Error: {stderr_string}");
        }

        break mbrola_output.stdout;
    };

    // Fix the wav header to set the ChunkSize and SubChunk2Size
    // See:
    // - https://github.com/hadware/voxpopuli/blob/fb94a6130c046bb9f7a27aaaed2a4b434666faa9/voxpopuli/main.py#L150-L158
    // - http://soundfile.sapp.org/doc/WaveFormat/
    let wav_len: u32 = raw_wav.len().try_into().expect("WAV data too long!");

    raw_wav[4..8].copy_from_slice(&(wav_len - 8).to_le_bytes());
    raw_wav[40..44].copy_from_slice(&(wav_len - 44).to_le_bytes());

    Ok((
        bytes::Bytes::from(raw_wav),
        Some(HeaderValue::from_static("audio/wav")),
    ))
}

pub fn check_length(audio: &[u8], max_length: u32) -> bool {
    audio.len() as u32
        / (u16::from_le_bytes(audio[22..24].try_into().unwrap()) as u32 * // Sample Rate
        u32::from_le_bytes(audio[24..28].try_into().unwrap()) *        // Number of Channels
        u16::from_le_bytes(audio[34..36].try_into().unwrap()) as u32   // Bits per Sample
        / 8)
        < max_length
}

pub fn get_voices() -> &'static [String] {
    static VOICES: OnceLock<Vec<String>> = OnceLock::new();
    VOICES.get_or_init(|| {
        (|| {
            let mut files = Vec::new();
            for file in std::fs::read_dir(aformat!("{}/voices/mb", astr!(ESPEAK_NG_DATA_PATH)))? {
                let file = file?;
                if file.file_type()?.is_file() {
                    let file_name = file.file_name().into_string().expect("Invalid filename!");
                    let mut file_name_iter = file_name.split('-').skip(1);

                    if let Some(language) = file_name_iter.next()
                        && file_name_iter.next().is_none()
                    {
                        files.push(language.to_owned());
                    }
                }
            }

            files.sort();
            anyhow::Ok(files)
        })()
        .unwrap()
    })
}

pub fn check_voice(voice: &str) -> bool {
    get_voices().iter().any(|s| s.as_str() == voice)
}
