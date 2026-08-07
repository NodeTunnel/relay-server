# NodeTunnel Relay Server

A lightweight, Rust-based relay server designed for Godot multiplayer games.

## Setup:
```bash
git clone https://github.com/NodeTunnel/relay-server.git
cd relay-server
cargo build --release

#Linux
RELAY_ID=LOCAL ALLOWED_VERSIONS=1.1.0_beta UDP_BIND_ADDRESS=0.0.0.0:8080 ./relay_server

#Windows
set RELAY_ID=LOCAL
set ALLOWED_VERSIONS=1.1.0_beta 
set UDP_BIND_ADDRESS=0.0.0.0:8080 
relay_server.exe
```

## Alternatively you can run it through docker
```bash
docker compose up -d --build
```

## Configuration:
```bash
cp .env.example .env
```
Then edit the `.env` as required. Once you have an app ID you should add it to the `WHITELIST`. (this is also where you chnage the port).

---

Dont forget to portforward the used port (default:8080/udp) on the device hosting this server (which may require using an external panel for hosting services such as AWS, Oracle etc.)
