FROM lukemathwalker/cargo-chef:latest-rust-latest AS chef

ENV RUSTFLAGS="-C target-cpu=native"
ARG MODES="espeak"

WORKDIR /build

# Container to generate a recipe.json
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Container to build the bot
FROM chef AS builder

# Install required dependencies
RUN bash -c 'if [[ "$MODES" == *"espeak"* ]]; then \
    apt-get update && \
    apt-get install -y libclang-dev libespeak-ng1 && \
    rm -rf /var/lib/apt/lists/*;  \
fi'

# This is a dummy build to get the dependencies cached.
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --no-default-features --features $MODES

# This is the actual build, copy in the rest of the sources
COPY . .
RUN cargo build --release --no-default-features --features $MODES

# Now make the runtime container
FROM debian:buster-slim

ARG MODES="espeak"

RUN bash -c '\
    apt-get update && \
    apt-get upgrade && \
    apt-get install -y openssl ca-certificates && \
    if [[ "$MODES" == *"espeak"* ]]; then \
        apt-get install -y git subversion espeak-ng make gcc && \
        git clone https://github.com/numediart/MBROLA && \
        cd MBROLA && \
        make && \
        cp Bin/mbrola /usr/bin/mbrola && \
        cd .. && \
        rm -rf MBROLA && \
        svn export https://github.com/numediart/MBROLA-voices/trunk/data /usr/share/mbrola; \
    fi; \
    rm -rf /var/lib/apt/lists/*'

COPY --from=builder /build/target/release/tts-service /usr/local/bin/tts-service
COPY Cargo.lock .

CMD ["/usr/local/bin/tts-service"]
