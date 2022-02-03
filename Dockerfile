FROM rust as builder
ENV RUSTFLAGS="-C target-cpu=native"

WORKDIR /build

# This is a dummy build to get the dependencies cached.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && \
    echo "// dummy file" > src/lib.rs && \
    cargo build --release && \
    rm -r src

# This is the actual build, copy in the rest of the sources
COPY . .
RUN cargo build --release

# Now make the runtime container
FROM debian:buster-slim

RUN apt-get update && apt-get upgrade && apt-get install -y openssl ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/gtts-service /usr/local/bin/gtts-service
COPY Cargo.lock .

CMD ["/usr/local/bin/gtts-service"]
