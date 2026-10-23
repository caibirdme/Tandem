FROM ubuntu:22.04

ARG RUST_VERSION

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        build-essential ca-certificates curl git libssl-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/opt/rust/rustup
ENV CARGO_HOME=/opt/rust/cargo
ENV PATH="/opt/rust/cargo/bin:${PATH}"

RUN test -n "${RUST_VERSION}" \
    && curl --proto '=https' --tlsv1.2 --fail --silent --show-error \
        https://sh.rustup.rs --output /tmp/rustup-init.sh \
    && sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain "${RUST_VERSION}" \
    && rm /tmp/rustup-init.sh

ENV RUSTUP_TOOLCHAIN=${RUST_VERSION}
ENV CARGO_HOME=/cargo-cache
ENV CARGO_TARGET_DIR=/build-target

WORKDIR /workspace
