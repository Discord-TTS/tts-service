FROM lukemathwalker/cargo-chef:latest-rust-latest AS chef

WORKDIR /build

# Container to generate a recipe.json
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Container to build the bot
FROM chef AS builder

# This is a dummy build to get the dependencies cached.
COPY --from=planner /build/recipe.json recipe.json
RUN apt-get update && apt-get upgrade -y && apt-get install -y cmake && cargo chef cook --release

# This is the actual build, copy in the rest of the sources
COPY . .
RUN cargo build --release

# Now make the runtime container
FROM debian:trixie-slim AS runtime

COPY sparse-checkout.sh .

RUN apt-get update && apt-get upgrade -y && \
    apt-get install -y cmake git make g++ tini && \
    apt-get clean && \
    # Build and install espeak-ng
    git clone https://github.com/espeak-ng/espeak-ng --depth 1 && cd espeak-ng && \
    cmake . && make -j && make install && \
    cd .. && rm -rf espeak-ng && mv /usr/local/lib/libespeak* /usr/lib && \
    # Build and install mbrola
    git clone https://github.com/numediart/MBROLA --depth 1 && cd MBROLA && make -j && cp Bin/mbrola /usr/bin/mbrola && cd .. && rm -rf MBROLA && \
    # Download the mbrola voices to /usr/share/mbrola.
    ./sparse-checkout.sh https://github.com/numediart/MBROLA-voices /usr/share/mbrola && mv /usr/share/mbrola/data/* /usr/share/mbrola && rm -r /usr/share/mbrola/data

COPY --from=builder /build/target/release/tts-service /usr/local/bin/tts-service
COPY Cargo.lock .

CMD ["/usr/bin/tini", "/usr/local/bin/tts-service"]
