# Shopping Sessions: a single static binary, built from source.
#
# Stage 1 compiles fast/ for x86-64 (musl, fully static, linked with the
# toolchain's own lld). Stage 2 is an empty image holding only that binary, so
# container start costs nothing beyond exec.
FROM rust:1.100.0-slim AS build
RUN rustup target add x86_64-unknown-linux-musl
WORKDIR /src
COPY fast/Cargo.toml fast/Cargo.lock ./
COPY fast/.cargo ./.cargo
COPY fast/src ./src
RUN cargo build --release --locked \
 && cp target/x86_64-unknown-linux-musl/release/sessions /sessions

FROM scratch
COPY --from=build /sessions /sessions
# The scorer runs as uid 1000 with a read-only root; everything is written to
# $OUTPUT_DIR (default /output). No arguments: DATA_DIR/OUTPUT_DIR/NCPU come from the environment.
USER 1000:1000
CMD ["/sessions"]
