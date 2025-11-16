FROM rust:latest as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/relay-server /usr/local/bin/app
COPY config.toml /config.toml
WORKDIR /
EXPOSE 8080/udp
CMD ["app"]