# gTTS-service

HTTP microservice using Axum and Reqwest to request the Google Translate TTS endpoint without rate limits

## Enviroment Variables (default)
- `IPV6_BLOCK` - A block of IPv6 addresses, randomly selected for each request
 
- `LOG_LEVEL`(`INFO`) - The lowest log level to output to stdout
- `BIND_ADDR`(`0.0.0.0:3000`) - The address to bind the web server to
