## Builder

FROM rust:latest as builder
ARG NAME=hermes
ARG TARGET=x86_64-unknown-linux-musl

# Set up
RUN rustup target add $TARGET
RUN apt update && apt install -y musl-tools musl-dev

# Create work dir
RUN USER=root cargo new --bin $NAME
WORKDIR $NAME

# Pre-build deps
COPY Cargo.toml .
RUN cargo build --release --target $TARGET
RUN rm src/*.rs

# Copy source code
COPY src src
RUN touch src/main.rs

# Build executable
RUN cargo build --release --target $TARGET && mv target/$TARGET/release/$NAME /app

## Runner image

FROM scratch

# Copy executable
COPY --from=builder /app /

ENTRYPOINT ["/app"]
