FROM rust:1.85-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/relay-server /usr/local/bin/app
WORKDIR /
RUN useradd --system --uid 10001 relay
USER relay
EXPOSE 8080/udp
ENTRYPOINT ["/usr/local/bin/app"]
