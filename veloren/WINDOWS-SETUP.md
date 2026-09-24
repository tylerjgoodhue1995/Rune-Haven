# Windows Release Build and Setup Guide

## Building Release Binaries

### Prerequisites
- Rust toolchain installed
- Git LFS initialized (for assets)
- Windows PowerShell

### Build Process

1. **Set up Git LFS** (if not already done):
   ```powershell
   git lfs install
   git lfs pull
   ```

2. **Build release binaries**:
   ```powershell
   .\build-release.bat
   ```

   This will:
   - Build server-cli, voxygen, auth service, and marketplace service
   - Set the `VELOREN_USERDATA_STRATEGY=executable` for proper asset handling
   - Create release binaries in `target\release\`

3. **Package for distribution**:
   ```powershell
   .\package-release.bat
   ```

   This will:
   - Create a `veloren-release` folder
   - Copy all binaries and assets
   - Set up proper directory structure
   - Include configuration templates and documentation

## Asset Folder Fix

The asset folder issue in release builds has been fixed by:

1. **Build Configuration**: 
   - Set `VELOREN_USERDATA_STRATEGY=executable` during build
   - This makes the executable look for assets in its own directory

2. **Runtime Scripts**:
   - Updated PowerShell scripts to check for assets in multiple locations
   - Added fallback logic to find assets folder
   - Scripts now set `VELOREN_ASSETS` environment variable explicitly

3. **Packaging**:
   - Assets are copied to `target\release\assets` during build
   - Package script includes the entire assets folder
   - Release bundle is self-contained

## VPS Deployment

### Pre-configured Settings

The release package comes pre-configured for your VPS at **52.247.50.106**:

- **Auth Server URL**: `http://52.247.50.106:19253`
- **Game Server Binding**: `0.0.0.0:14004` (all interfaces)
- **Query Server**: `0.0.0.0:14006`

### Deployment Steps

1. **Upload to VPS**:
   - Upload the entire `veloren-release` folder to your VPS
   - Recommended location: `/opt/veloren/` or `C:\veloren\` on Windows VPS

2. **Configure Environment**:
   ```powershell
   cd veloren-release
   copy config\.env.example .env
   copy config\auth.env.example auth.env
   copy config\marketplace.env.example marketplace.env
   ```

3. **Edit Configuration Files**:
   - Update `.env` with your specific settings
   - Update `auth.env` with auth service configuration
   - Update `marketplace.env` with marketplace settings

4. **Set Up Port Forwarding**:
   - Forward ports 14004, 14005, 14006 on your VPS
   - Keep 19253, 19254 private (see NETWORKING.md)
   - Configure firewall rules

5. **Start Services**:
   ```powershell
   # Terminal 1 - Auth Service
   Set-ExecutionPolicy -Scope Process Bypass
   .\start-auth.ps1

   # Terminal 2 - Marketplace Service
   Set-ExecutionPolicy -Scope Process Bypass
   .\start-marketplace.ps1

   # Terminal 3 - Game Server
   Set-ExecutionPolicy -Scope Process Bypass
   .\start-game-server.ps1
   ```

## Required Ports

### Public Ports (for players)
- **14004/tcp** - Main game server
- **14005/tcp** - Alternative game server
- **14006/udp** - Query server

### Private Ports (internal services)
- **19253/tcp** - Auth service (keep private or use HTTPS reverse proxy)
- **19254/tcp** - Marketplace service (keep private)

See [NETWORKING.md](vps-deploy/NETWORKING.md) for detailed configuration.

## Troubleshooting

### "Asset folder cannot be found" Error

If you still encounter asset folder issues:

1. **Check asset location**:
   ```powershell
   dir assets
   ```

2. **Set environment variable manually**:
   ```powershell
   $env:VELOREN_ASSETS = "C:\path\to\veloren-release\assets"
   ```

3. **Verify canary file**:
   ```powershell
   type assets\common\canary.canary
   ```
   Should start with "VELOREN_CANARY_MAGIC"

### Server Won't Start

1. **Check port availability**:
   ```powershell
   netstat -an | findstr "14004"
   ```

2. **Review logs** in `userdata\server\` and `userdata\server-cli\`

3. **Verify configuration** in `.env` and `userdata\server\server_config\settings.ron`

### Players Cannot Connect

1. **Verify firewall rules** on VPS
2. **Check VPS provider security groups**
3. **Test connectivity**: `telnet 52.247.50.106 14004`
4. **Ensure server is binding to 0.0.0.0 not 127.0.0.1`

## Development vs Production

### Development Build
```powershell
cargo run --bin veloren-server-cli
```
- Uses assets from repository root
- Hot reloading enabled
- Debug symbols included

### Release Build
```powershell
.\build-release.bat
```
- Optimized for performance
- Assets bundled with executable
- Smaller binary size (compressed debug info)
- Production-ready configuration

## Additional Resources

- [VPS Deployment Guide](vps-deploy/README.md)
- [Networking Configuration](vps-deploy/NETWORKING.md)
- [Veloren Book](https://book.veloren.net)
- [Veloren Discord](https://veloren.net/discord)
