use tokio::io::AsyncReadExt;

use crate::Error;


pub(crate) async fn get_tts(text: &str, voice: &str) -> std::io::Result<Vec<u8>> {
    // We have to loop due to random "unable to get .wav header" errors.
    loop {
        let espeak_process = tokio::process::Command::new("espeak")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .args(["--pho", "-q", "-v", &format!("mb/mb-{}", voice), text])
            .spawn()?;

        let tokio::process::Child{stdout, stderr, ..} = espeak_process;

        let espeak_stdout: std::process::Stdio = stdout
            .expect("Failed to open espeak stdout")
            .try_into()?;

        let mbrola_process = tokio::process::Command::new("mbrola")
            .stdout(std::process::Stdio::piped())
            .stdin(espeak_stdout)
            .args(["-e", &format!("/usr/share/mbrola/{voice}/{voice}", voice=voice), "-", "-.wav"])
            .spawn()?;

        let output = mbrola_process.wait_with_output().await?;
        if output.stdout.len() == 44 {
            let mut espeak_stderr = stderr.expect("Unable to open espeak stderr");

            let mut stderr = Vec::new();
            espeak_stderr.read_to_end(&mut stderr).await?;

            if String::from_utf8(stderr).unwrap().contains("mbrowrap error: unable to get .wav header from mbrola") {
                continue
            }
        };

        break Ok(output.stdout)
    }
}

pub(crate) fn get_voices() -> Vec<String> {
    || -> Result<_, Error> {
        let (_, mut voice_path) = espeakng::Speaker::info();
        voice_path.push("voices/mb");

        let mut files = Vec::new();
        for file in std::fs::read_dir(voice_path)? {
            let file = file?;
            if file.file_type()?.is_file() {
                let file_name = file.file_name().into_string().expect("Invalid filename!");
                files.push(file_name.split('-').last().unwrap().to_owned());
            }
        };

        Ok(files)
    }().unwrap()
}
