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

# Build and install espeak-ng
RUN apt-get update && apt-get install -y libclang-dev && apt-get clean && \ 
    git clone https://github.com/espeak-ng/espeak-ng --depth 1 && cd espeak-ng && \
    ./autogen.sh && ./configure --prefix=/usr && make && make install && \ 
    cd .. && rm -rf espeak-ng

# This is a dummy build to get the dependencies cached.
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --no-default-features --features $MODES

# This is the actual build, copy in the rest of the sources
COPY . .
RUN cargo build --release --no-default-features --features $MODES

# Now make the runtime container
FROM debian:bullseye-slim

RUN apt-get update && apt-get upgrade && \
    apt-get install -y openssl ca-certificates git subversion make autoconf automake libtool pkg-config g++ && \
    apt-get clean && \
    # Build and install espeak-ng
    git clone https://github.com/espeak-ng/espeak-ng --depth 1 && cd espeak-ng && \
    ./autogen.sh && ./configure && make && make install && \ 
    cd .. && rm -rf espeak-ng && mv /usr/local/lib/libespeak* /usr/lib && \
    # Build and install mbrola
    git clone https://github.com/numediart/MBROLA && cd MBROLA && make && cp Bin/mbrola /usr/bin/mbrola && cd .. && rm -rf MBROLA && \
    # Download the mbrola voices to /usr/share/mbrola.
    svn export https://github.com/numediart/MBROLA-voices/trunk/data /usr/share/mbrola

COPY --from=builder /build/target/release/tts-service /usr/local/bin/tts-service
COPY Cargo.lock .

CMD ["/usr/local/bin/tts-service"]
