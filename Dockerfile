FROM rust as builder

ENV RUSTFLAGS="-C target-cpu=native"
ARG MODES="espeak"

WORKDIR /build

# This is a dummy build to get the dependencies cached.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && \
    echo "// dummy file" > src/lib.rs && \
    cargo build --release --features $MODES && \
    rm -r src

# This is the actual build, copy in the rest of the sources
COPY . .
RUN cargo build --release --features $MODES

# Now make the runtime container
FROM debian:buster-slim

RUN apt-get update && \
    apt-get upgrade && \
    apt-get install -y openssl ca-certificates git subversion espeak-ng make gcc && \
    rm -rf /var/lib/apt/lists/* && \
    # Install mbrola
    git clone https://github.com/numediart/MBROLA && cd MBROLA && \
    make && cp Bin/mbrola /usr/bin/mbrola && \
    cd .. && rm -rf MBROLA && \
    # Install mbrola voices
    svn export https://github.com/numediart/MBROLA-voices/trunk/data /usr/share/mbrola

COPY --from=builder /build/target/release/tts-service /usr/local/bin/tts-service
COPY Cargo.lock .

CMD ["/usr/local/bin/tts-service"]
