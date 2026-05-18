# Homepage Deployment

`sealport.cc` is served by `sealport-web`, a lightweight Rust binary that uses
Axum and server-rendered Leptos views. This binary is public marketing
infrastructure only; it is not a SealPort backup server, daemon, scheduler, or
hosted product.

## Local Run

```sh
cargo run -p sealport-web
```

The default listener is `0.0.0.0:8080`. Override it with:

```sh
SEALPORT_WEB_ADDR=127.0.0.1:8096 cargo run -p sealport-web
```

`GET /healthz` returns `ok` for reverse-proxy and process supervision checks.

## Ubuntu Shape

Build the binary on the server or copy a release artifact once release
packaging exists:

```sh
cargo build --release -p sealport-web
sudo install -m 0755 target/release/sealport-web /usr/local/bin/sealport-web
```

The production host uses the repo-owned templates under `deploy/` and binds
the homepage to `127.0.0.1:8096` so it does not conflict with other local Rust
sites.

Create a dedicated unprivileged user:

```sh
sudo useradd --system --home /opt/sealport-web --shell /usr/sbin/nologin sealport-web
```

Example systemd unit:

```ini
[Unit]
Description=SealPort public homepage
After=network-online.target
Wants=network-online.target

[Service]
User=sealport-web
Group=sealport-web
Environment=SEALPORT_WEB_ADDR=127.0.0.1:8096
ExecStart=/opt/sealport-web/sealport-web
Restart=on-failure
RestartSec=5s
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/sealport-web

[Install]
WantedBy=multi-user.target
```

Install and start it:

```sh
sudo install -m 0644 deploy/systemd/sealport-web.service /etc/systemd/system/sealport-web.service
sudo systemctl daemon-reload
sudo systemctl enable --now sealport-web
curl -fsS http://127.0.0.1:8096/healthz
```

## Reverse Proxy

Terminate TLS at the reverse proxy and forward to the local listener.

Example Caddy site:

```caddyfile
sealport.cc {
	reverse_proxy 127.0.0.1:8096
}
```

Example nginx server:

```nginx
server {
    listen 80;
    server_name sealport.cc www.sealport.cc;

    location / {
        proxy_pass http://127.0.0.1:8096;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Use the proxy's normal TLS automation or certificate deployment path before
serving production traffic.
