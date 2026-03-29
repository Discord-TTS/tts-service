use std::fmt::{Debug, Display};

use songbird::input::{AudioStream, AudioStreamError, core::io::MediaSource};

use crate::GetTTS;

pub(super) struct TTSSource(pub Option<GetTTS>);

#[derive(Debug)]
struct ErrWrapper<T>(T);

impl<T: Display> Display for ErrWrapper<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: Debug + Display> std::error::Error for ErrWrapper<T> {}

#[async_trait::async_trait]
impl songbird::input::Compose for TTSSource {
    fn should_create_async(&self) -> bool {
        true
    }

    fn create(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        Err(AudioStreamError::Unsupported)
    }

    async fn create_async(
        &mut self,
    ) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        let request = self.0.take().ok_or(AudioStreamError::Unsupported)?;
        match crate::get_tts_inner(crate::STATE.get().unwrap(), request).await {
            Ok((audio, _)) => {
                let input = Box::new(std::io::Cursor::new(audio));
                Ok(AudioStream { input, hint: None })
            }
            Err(err) => Err(AudioStreamError::Fail(ErrWrapper(err).into())),
        }
    }
}
