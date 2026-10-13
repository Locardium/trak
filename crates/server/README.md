# trak-server

Trak's archive and sync server. Stores whatever devices send it and hands it
back when they ask for it. **It doesn't import music on its own and has no
interface**: everything it has, someone sent it.

What it's for, in two concrete cases:

- **Syncing away from home.** mDNS discovery only sees the local network, and
  two devices on different networks can't call each other either: both are
  behind a NAT. Against a server with a public address, on the other hand,
  both dial out. And since the server has everything, neither needs the other
  to be online.
- **Recovering.** If every device's library is lost, the files and the
  organization are still here.

## Quick start on Linux

No Rust needed: grab the binary from the latest release.

```
sudo useradd --system --home /var/lib/trak --create-home trak
sudo curl -L -o /usr/local/bin/trak-server   https://github.com/Locardium/trak/releases/latest/download/trak-server-linux
sudo chmod +x /usr/local/bin/trak-server
cd /var/lib/trak && sudo -u trak /usr/local/bin/trak-server
```

That first run writes `/var/lib/trak/config.toml` with a new `pair_token` and
exits. Review it (see [Running](#running)), then install the
[systemd unit](#systemd) so it starts on boot, and open port 7420 on the
router. In the app: Sync → add server → the address and the `pair_token`.

Prefer containers? See [Docker](#docker).

## Building

```
cargo build --release -p trak-server
```

The binary ends up in `target/release/trak-server` (`.exe` on Windows). It
doesn't depend on anything from the system: no graphical environment, no
webview, no SQLite installation needed.

### For Linux, from Windows

A binary works for a single operating system: the `.exe` won't run on the
server. This builds the Linux one without leaving Windows and without
installing anything on the server — no Rust, no source code.

Once:

```
winget install zig.zig
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-musl
```

Each time:

```
cargo zigbuild --release -p trak-server --target x86_64-unknown-linux-musl
```

A single file comes out at
`target/x86_64-unknown-linux-musl/release/trak-server`, and it's the only
thing that needs to be copied over.

**musl and not gnu** on purpose: the binary ends up statically linked, with
SQLite bundled in, so it doesn't matter what glibc version the server has or
which distribution it is. With `gnu`, a binary built against a newer glibc
than the server's won't start — and the error won't say that, it'll say it
can't find the file.

`zig` here isn't the language: it's its C compiler, which knows how to target
another system. It's needed because SQLite ships as C code and has to be
compiled for the target.

## Running

```
./trak-server [config-path]
```

With no argument it uses `config.toml` from the current directory. The first
run writes that file with a new token and exits, so you can review it before
opening the port. (Unless `TRAK_SERVER_TOKEN` is set: then there is nothing
to copy out of the file, so it is written with defaults and an empty
`pair_token`, and the server starts right away.) The same file, commented, is
in [`config.example.toml`](config.example.toml):

```toml
listen = "0.0.0.0:7420"
name = "Trak Server"
data_dir = "data"
music_dir = "music"
retention_days = 90
pair_token = "..."
```

- `music_dir` is what grows: point it at the big disk, not the system one.
  **On Windows a backslash is an escape in TOML**: use `'D:\Music'` with
  single quotes, or `"D:/Music"` with forward slashes.
- `retention_days` is how many days a deleted file survives in the server's
  trash. A deletion travels: you delete it on one device and it disappears
  from all of them, server included. This is the only thing that makes it
  recoverable afterward. At `0` it's destroyed on the spot — an exact mirror
  of your devices, with no safety net.
- `pair_token` is what you type into the app to pair a device. It replaces
  the six-digit code, because there's no screen here to compare it against.
  **Treat it like a password.** Changing it (and restarting the server)
  unpairs every device paired with the old one: on its next connection the
  app is told it's no longer paired, forgets the server, and has to pair
  again with the new token. It can also be passed via the
  `TRAK_SERVER_TOKEN` environment variable, which overrides the file's value:
  in a deployment it's best for the secret not to live in a file that could
  end up in a repo.

### Versions

The server and the apps speak a versioned sync protocol. The startup log says
which one (`protocol 3 (accepts 3+)`). When a device connects with a protocol
out of range, nothing syncs and both sides say who has to update: the app
shows **Update needed** on the server, and the server logs a line like
`... runs a newer Trak (...): update trak-server` or
`... runs an older Trak (...): update the app on that device`.

## What it stores

Everything. The server doesn't pick and choose: each device sends it what it
has and takes back what it's missing. The same song sent by three devices
takes up **only one copy** — files are identified by their content, not by
their name.

That's why its sync configuration can't be edited from the app: a file with
holes isn't a file, and a server set to "send only" won't give you anything
back the day you need it.

## Change notifications

Sync is always driven by the device: the server just responds and doesn't
call anyone —it can't, and that's on purpose: this way devices don't need to
be reachable from the internet—. So a change made away from home isn't slow
to be noticed, each device leaves **one connection open, waiting**, and the
server answers over it as soon as its library moves. Whoever was waiting
syncs; whoever caused the change isn't notified.

This matters for anything sitting in the middle (a TCP proxy, a NAT): those
connections stay quiet almost all the time. The server sends a heartbeat
every 25 seconds to keep them alive, so any inactivity timeout along the way
has to be longer than that — the default for Nginx Proxy Manager's Streams is
10 minutes, which is more than enough.

Twenty-five seconds and no more because the deadline that matters isn't the
proxy's: it's the carrier's mobile-data NAT, which drops anything quiet for
between 30 and 60 seconds. The connection dropping anyway isn't a big deal
—the device reconnects saying which revision it knew about and the server
tells it whether it missed something—, but each drop leaves the server
talking to a dead connection for a while until its heartbeat fails. It's
also what lets the app notice a server that vanished without closing the
connection: it gives up after 60 seconds without a heartbeat and shows the
server as unreachable.

A server older than this cuts the connection when it receives the wait
request. The app detects that after three attempts and falls back to
periodic polling, so nothing breaks; it's just that remote changes take as
long as they used to. It's best to update the server before the app.

## Security

- All traffic goes over an encrypted channel (Noise XX over TCP, the same one
  devices use with each other). No need to put anything in front of it or set
  up TLS.
- A device with a key different from the one the server already had for that
  uid **is rejected and logged**, whatever token it presents. Pairing it
  again requires unpairing it first, by hand.
- The port needs to be reachable from outside: port forwarding on the router
  or a public IP. That's the only thing that needs to be exposed.

## Docker

The image runs the same binary, as a non-root user, with everything it keeps
(`config.toml`, `data/`, `music/`) in the `/srv/trak` volume. From this
folder, with Docker Compose ([`docker-compose.yml`](docker-compose.yml)):

```
echo "TRAK_SERVER_TOKEN=$(openssl rand -hex 24)" > .env
docker compose up -d
```

The token in `.env` is what you type in the app to pair. The library ends up
in `./trak-data` next to the compose file; change that line to put it on the
big disk. No `network_mode: host` needed: the server doesn't use mDNS
(devices add it by address), only port 7420.

Without Compose, from the repo root:

```
docker build -f crates/server/Dockerfile -t trak-server .
docker run -d --name trak-server --restart unless-stopped   -p 7420:7420 -e TRAK_SERVER_TOKEN=... -v /srv/trak:/srv/trak trak-server
```

## systemd

```ini
[Unit]
Description=Trak sync server
After=network-online.target

[Service]
User=trak
WorkingDirectory=/var/lib/trak
ExecStart=/usr/local/bin/trak-server /var/lib/trak/config.toml
Restart=on-failure

[Install]
WantedBy=multi-user.target
```
