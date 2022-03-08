# TTS-service

HTTP microservice using Axum to generate TTS from an HTTP reqwest.

## Modes
- eSpeak - Local TTS, low quality. Returns WAV audio.
- gTTS - Cloud TTS, medium quality. Returns MP3 audio
- Premium - Cloud TTS, high quality. Returns OPUS audio. **Requires a gCloud API key**

## Supported endpoints:
- `GET /tts?text={CONTENT}&voice={VOICE}&mode={MODE}` - Returns the audio generated. 
- `GET /voices?mode={MODE}` - Returns the supported voices for the given mode as a JSON array of strings.
- `GET /modes` - Returns the currently supported modes for TTS as a JSON array of strings.

It is undefined what body non-200 requests will return, if any.

## Environment Variables (default)
- `IPV6_BLOCK` - A block of IPv6 addresses, randomly selected for each gTTS request

- `GOOGLE_APPLICATION_CREDENTIALS` - The file path to the gCloud JSON

- `BIND_ADDR`(`0.0.0.0:3000`) - The address to bind the web server to

- `REDIS_URI` - The URI of a redis instance to cache requests with

- `CACHE_KEY` - Fernet encryption key to use to encrypt audio data

- `LOG_LEVEL`(`INFO`) - The lowest log level to output to stdout

## Docker build variables (default)
- `MODES`(`espeak`) - A comma separated list of modes to support
