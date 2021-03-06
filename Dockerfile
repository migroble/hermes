## Builder

FROM rust:latest as builder
ARG NAME=hermes
ARG TARGET=x86_64-unknown-linux-musl
ARG OPENSSL_VERSION=1_1_1k

# Set up
RUN rustup target add $TARGET
RUN apt-get update && apt-get install -y musl-tools musl-dev

# Compile OpenSSL statically
# Based on https://qiita.com/liubin/items/6c94f0b61f746c08b74c
RUN ln -s /usr/include/x86_64-linux-gnu/asm /usr/include/x86_64-linux-musl/asm && \
    ln -s /usr/include/asm-generic /usr/include/x86_64-linux-musl/asm-generic && \
    ln -s /usr/include/linux /usr/include/x86_64-linux-musl/linux && \
    mkdir /musl && \
    wget https://github.com/openssl/openssl/archive/refs/tags/OpenSSL_$OPENSSL_VERSION.tar.gz && \
    tar zxvf OpenSSL_$OPENSSL_VERSION.tar.gz
ENV CC="musl-gcc -fPIE -pie"
WORKDIR openssl-OpenSSL_$OPENSSL_VERSION
RUN ./Configure no-shared no-async --prefix=/musl --openssldir=/musl/ssl linux-x86_64
RUN make depend && make -j$(nproc) && make install

ENV PKG_CONFIG_ALLOW_CROSS=1
ENV OPENSSL_STATIC=1
ENV OPENSSL_DIR=/musl
ENV LIBZ_SYS_STATIC=1
WORKDIR / 

# Create work dir
RUN cargo new --bin $NAME
WORKDIR $NAME

# Pre-build deps
COPY Cargo.toml .
RUN cargo build --features mimalloc --release --target $TARGET
RUN rm src/*.rs

# Copy source code
COPY src src
RUN touch src/main.rs

# Build executable
RUN cargo build --features mimalloc --release --target $TARGET && mv target/$TARGET/release/$NAME /app

## Runner image

FROM scratch

# Copy executable
COPY --from=builder /app /

ENTRYPOINT ["/app"]
