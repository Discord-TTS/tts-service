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
RUN cargo chef cook --release

# This is the actual build, copy in the rest of the sources
COPY . .
RUN cargo build --release

# Now make the runtime container
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get upgrade -y && \
    apt-get install -y openssl ca-certificates git subversion make autoconf automake libtool pkg-config g++ && \
    apt-get clean && \
    # Build and install espeak-ng
    git clone https://github.com/espeak-ng/espeak-ng --depth 1 && cd espeak-ng && \
    ./autogen.sh && ./configure && make && make install && \ 
    cd .. && rm -rf espeak-ng && mv /usr/local/lib/libespeak* /usr/lib && \
    # Build and install mbrola
    git clone https://github.com/numediart/MBROLA --depth 1 && cd MBROLA && make && cp Bin/mbrola /usr/bin/mbrola && cd .. && rm -rf MBROLA && \
    # Download the mbrola voices to /usr/share/mbrola.
    svn export https://github.com/numediart/MBROLA-voices/trunk/data /usr/share/mbrola

# Download tini to avoid zombie processes
ADD https://github.com/krallin/tini/releases/latest/download/tini /usr/local/bin/tini
RUN chmod +x /usr/local/bin/tini

COPY --from=builder /build/target/release/tts-service /usr/local/bin/tts-service
COPY Cargo.lock .

CMD ["/usr/local/bin/tini", "/usr/local/bin/tts-service"]
