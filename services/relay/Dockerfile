FROM rust:1.96-bookworm AS build
WORKDIR /build
COPY services/Cargo.toml services/Cargo.lock ./services/
COPY services/protocol ./services/protocol
COPY services/remote ./services/remote
COPY services/relay ./services/relay
RUN cargo build --locked --release --manifest-path services/Cargo.toml -p vibra-relay
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/services/target/release/vibra-relay /usr/local/bin/vibra-relay
USER 65532:65532
ENV VIBRA_RELAY_BIND=0.0.0.0:8787
EXPOSE 8787
ENTRYPOINT ["/usr/local/bin/vibra-relay"]
