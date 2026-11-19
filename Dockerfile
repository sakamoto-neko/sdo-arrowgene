FROM cgr.dev/chainguard/wolfi-base:latest AS build
RUN apk add --no-cache rust-1.95 build-base
WORKDIR /src/app
COPY app/Cargo.toml app/Cargo.lock ./
COPY app/crates ./crates
COPY web /src/web
RUN cargo build --locked --release -p sdo-server --bin sdo-server

# CI supplies this stage as a named context containing the native release binaries.
FROM scratch AS server-binaries
ARG TARGETARCH
COPY --from=build /src/app/target/release/sdo-server /${TARGETARCH}/sdo-server

FROM cgr.dev/chainguard/wolfi-base:latest
ARG TARGETARCH
RUN apk add --no-cache libgcc
COPY --from=server-binaries /${TARGETARCH}/sdo-server /usr/local/bin/sdo-server
USER 65532:65532
EXPOSE 32100/tcp 32101/tcp
ENTRYPOINT ["/usr/local/bin/sdo-server"]
