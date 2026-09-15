# Veloren VPS deployment bundle

This folder contains the runtime files and release binaries staged from this
development machine.

**Important:** the binaries currently in `bin/` are macOS Apple Silicon
(`arm64`) executables because this bundle was created on macOS. They will not
run on Windows. Build Windows binaries on Windows/CI, or replace these three
files with Windows-compatible `.exe` files before starting the services.

## Included

- `bin/veloren-server-cli` - custom game server
- `bin/veloren-beta-auth` - wallet/authentication service
- `bin/veloren-marketplace-service` - marketplace service
- `assets/` - server runtime assets, including the custom world-generation code's assets
- `userdata/` - current server and server-cli persistent data
- `auth-data/` - current authentication state
- `config/` - environment templates

No marketplace seller keypair is included. Put that secret in
`secrets/marketplace-seller.json` on the VPS and restrict it to mode `600`.

## Install

Copy this directory to the VPS, for example:

```text
/opt/veloren/
```

On Windows, install Rust (with the MSVC toolchain) and create the service
environment:

```bash
cd /opt/veloren
cp config/.env.example .env
cp config/auth.env.example auth.env
cp config/marketplace.env.example marketplace.env
```

From the full repository on Windows, build the custom binaries:

```powershell
$env:VELOREN_GIT_VERSION="/0/0"
cargo build --release -p veloren-server-cli
cargo build --release --manifest-path auth-service/Cargo.toml --bin veloren-beta-auth
cargo build --release -p veloren-marketplace-service
```

Copy these files into this bundle as `.exe` files:

```text
target\release\veloren-server-cli.exe
target\release\veloren-beta-auth.exe
target\release\veloren-marketplace-service.exe
```

Then copy `config\.env.example` to `.env`, `config\auth.env.example` to
`auth.env`, and `config\marketplace.env.example` to `marketplace.env`.

Replace every required placeholder. At minimum configure:

- `VELOREN_AUTH_SERVER_URL`
- `VELOREN_VGLD_MINT`
- `VELOREN_VGLD_TREASURY`
- marketplace seller, land mint, and seller keypair settings

Keep the VGLD property test-purchase flag disabled on a public server.

## Run manually on Windows

Open three PowerShell windows:

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\start-auth.ps1
```

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\start-marketplace.ps1
```

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\start-game-server.ps1
```

The older `.sh` scripts are for Linux/macOS and should not be used on Windows.

## Run manually on Linux

Open three terminals or create equivalent `systemd` services:

```bash
cd /opt/veloren
set -a; . ./auth.env; set +a
./bin/veloren-beta-auth
```

```bash
cd /opt/veloren
set -a; . ./marketplace.env; set +a
./bin/veloren-marketplace-service
```

```bash
cd /opt/veloren
set -a; . ./.env; set +a
VELOREN_AUTH_MODE=remote \
VELOREN_AUTH_SERVER_URL=http://127.0.0.1:19253 \
./bin/veloren-server-cli --non-interactive
```

The game server uses the working directory's `assets/` and `userdata/`
directories. Do not move the binary without moving those directories as well.

## Network

Expose only the game ports publicly:

- `14004/tcp`
- `14005/tcp`
- `14006/udp`

Keep `19253` and `19254` private unless they are placed behind an HTTPS reverse
proxy. The desktop wallet callback ports `38291` through `38293` are not VPS
service ports and must not be exposed publicly.

## Backups

Back up these paths before upgrades:

```text
userdata/server/
userdata/server-cli/
auth-data/
```

Do not commit `.env`, `auth.env`, `marketplace.env`, or any keypair under
`secrets/`.
