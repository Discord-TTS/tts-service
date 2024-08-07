use std::sync::{LazyLock, OnceLock};

use aformat::{aformat, CapStr, ToArrayString};
use memchr::memmem::Finder;
use reqwest::header::HeaderValue;
use tokio::io::AsyncReadExt;

use crate::Result;

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
    let mut stderr_buf = Vec::new();
    let mut raw_wav = loop {
        let espeak_process = tokio::process::Command::new("espeak")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .args([
                "--pho",
                "-q",
                "-s",
                &speaking_rate.to_arraystring(),
                "-v",
                &aformat!("mb/mb-{voice}"),
                text,
            ])
            .spawn()?;

        let tokio::process::Child { stdout, stderr, .. } = espeak_process;

        let espeak_stdout: std::process::Stdio =
            stdout.expect("Failed to open espeak stdout").try_into()?;

        let mut mbrola_process = tokio::process::Command::new("mbrola")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(espeak_stdout)
            .args([
                "-e",
                &aformat!("/usr/share/mbrola/{voice}/{voice}"),
                "-",
                "-.wav",
            ])
            .spawn()?;

        // Filter out some warning messages from mbrola that clutter logs
        if let Some(mut mbrola_stderr) = mbrola_process.stderr.take() {
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                while let Ok(written_bytes) = mbrola_stderr.read_buf(&mut buffer).await {
                    if written_bytes == 0 {
                        break;
                    }

                    if replaced_with_err.find(&buffer).is_none() {
                        tracing::error!("Mbrola Error: {}", String::from_utf8_lossy(&buffer));
                    }

                    buffer.clear();
                }

                tracing::debug!("mbrola_stderr watcher closed");
            });
        };

        let output = mbrola_process.wait_with_output().await?;
        if output.stdout.len() == 44 {
            let mut espeak_stderr = stderr.expect("Unable to open espeak stderr");

            stderr_buf.clear();
            espeak_stderr.read_to_end(&mut stderr_buf).await?;

            if repeat_err.find(&stderr_buf).is_some() {
                i += 1;
                continue;
            }
        };

        tracing::debug!("Generated eSpeak after {i} tries");
        break output.stdout;
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
            for file in std::fs::read_dir("/usr/local/share/espeak-ng-data/voices/mb")? {
                let file = file?;
                if file.file_type()?.is_file() {
                    let file_name = file.file_name().into_string().expect("Invalid filename!");
                    let mut file_name_iter = file_name.split('-').skip(1);

                    if let Some(language) = file_name_iter.next() {
                        if file_name_iter.next().is_none() {
                            files.push(language.to_owned());
                        }
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
