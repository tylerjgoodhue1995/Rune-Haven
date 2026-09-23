# Public beta VPS deployment

This deployment runs the game server, beta authentication service, and the
devnet marketplace with Docker Compose. It is intended for a small beta, not
production custody of valuable assets.

## VPS requirements

- Ubuntu 24.04 (or another current Docker-supported Linux distribution)
- 2+ vCPUs, 8 GB RAM, and at least 30 GB of disk
- A stable public IP or DNS name
- Docker Engine and the Compose plugin

Open only these ports in the VPS firewall/security group:

- `14004/tcp` and `14005/tcp` for the game server
- `14006/udp` for game traffic
- `80/tcp` and `443/tcp` if HTTPS will be used for the auth/marketplace API

Do not expose ports `19253` or `19254` publicly. Put those services behind a
reverse proxy with HTTPS and authentication/rate limiting appropriate for the
beta.

## Install and configure

```bash
sudo apt-get update
sudo apt-get install -y docker.io docker-compose-plugin
sudo usermod -aG docker "$USER"
newgrp docker

git clone <YOUR_PRIVATE_REPOSITORY_URL> /opt/veloren
cd /opt/veloren/server-cli
cp .env.beta.example .env
mkdir -p /opt/veloren/secrets beta-auth-data userdata
cp /path/to/marketplace-seller.json /opt/veloren/secrets/marketplace-seller.json
chmod 600 /opt/veloren/secrets/marketplace-seller.json
```

Edit `.env` and replace every `REPLACE_...` value. Set
`VELOREN_AUTH_SERVER_URL` to the public HTTPS URL used by clients. Keep the
seller keypair only on the VPS; never put it in `.env`, Git, or a container
image.

## Start and verify

```bash
docker compose -f docker-compose.beta.yml --env-file .env up -d --build
docker compose -f docker-compose.beta.yml ps
docker compose -f docker-compose.beta.yml logs -f beta-auth marketplace game-server
```

Verify the marketplace from the VPS itself:

```bash
curl http://127.0.0.1:19254/marketplace/listings
```

The response must contain the configured parcel and mint. Verify the auth
service through its HTTPS reverse-proxy URL before distributing the client.

## Client configuration

Build the client from the same commit as the server. Configure the public
HTTPS marketplace URL at build/run time:

```bash
export VELOREN_MARKETPLACE_API_URL=https://beta.example.com
```

Do not point public clients at `127.0.0.1`; that address means the player's
own computer.

## Operational notes

- Back up `beta-auth-data/auth-state.json` securely.
- Monitor disk usage and container logs.
- Pin the game-server image tag for a beta release instead of tracking
  `weekly` indefinitely.
- The current marketplace is custodial devnet infrastructure. Before handling
  real assets, replace it with an audited escrow/non-custodial design and add
  authenticated server-to-marketplace callbacks.
