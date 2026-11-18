FROM cgr.dev/chainguard/wolfi-base:latest AS build
RUN apk add --no-cache rust-1.95 build-base
WORKDIR /src
COPY app/Cargo.toml app/Cargo.lock ./
COPY app/crates ./crates
RUN cargo build --locked --release -p sdo-multiplayer --bin sdo-server

FROM cgr.dev/chainguard/wolfi-base:latest
RUN apk add --no-cache libgcc
COPY --from=build /src/target/release/sdo-server /usr/local/bin/sdo-server
USER 65532:65532
EXPOSE 32100/tcp
ENTRYPOINT ["/usr/local/bin/sdo-server"]
