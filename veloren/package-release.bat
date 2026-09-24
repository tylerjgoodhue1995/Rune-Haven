@echo off
REM Package script for Windows release builds with assets bundled

echo Packaging Veloren release for distribution...

REM Set release directory
set RELEASE_DIR=veloren-release
if exist %RELEASE_DIR% rmdir /s /q %RELEASE_DIR%
mkdir %RELEASE_DIR%

echo Creating release directory structure...

REM Create directory structure
mkdir %RELEASE_DIR%\bin
mkdir %RELEASE_DIR%\assets
mkdir %RELEASE_DIR%\userdata
mkdir %RELEASE_DIR%\userdata\server
mkdir %RELEASE_DIR%\userdata\server-cli
mkdir %RELEASE_DIR%\auth-data
mkdir %RELEASE_DIR%\config
mkdir %RELEASE_DIR%\secrets

echo Copying binaries...

REM Copy compiled binaries
copy target\release\veloren-server-cli.exe %RELEASE_DIR%\bin\
copy target\release\veloren-voxygen.exe %RELEASE_DIR%\bin\
copy target\release\veloren-beta-auth.exe %RELEASE_DIR%\bin\
copy target\release\veloren-marketplace-service.exe %RELEASE_DIR%\bin\

echo Copying assets (this may take a while)...

REM Copy assets folder
xcopy /E /I /Y assets %RELEASE_DIR%\assets

echo Copying configuration templates...

REM Copy configuration templates if they exist
if exist vps-deploy\config\.env.example copy vps-deploy\config\.env.example %RELEASE_DIR%\config\
if exist vps-deploy\config\auth.env.example copy vps-deploy\config\auth.env.example %RELEASE_DIR%\config\
if exist vps-deploy\config\marketplace.env.example copy vps-deploy\config\marketplace.env.example %RELEASE_DIR%\config\
if exist vps-deploy\config\settings.ron.example copy vps-deploy\config\settings.ron.example %RELEASE_DIR%\config\

echo Setting up server configuration...

REM Create server config directory and copy settings
mkdir %RELEASE_DIR%\userdata\server\server_config
if exist vps-deploy\config\settings.ron.example copy vps-deploy\config\settings.ron.example %RELEASE_DIR%\userdata\server\server_config\settings.ron

echo Copying startup scripts...

REM Copy startup scripts
copy vps-deploy\start-game-server.ps1 %RELEASE_DIR%\
copy vps-deploy\start-auth.ps1 %RELEASE_DIR%\
copy vps-deploy\start-marketplace.ps1 %RELEASE_DIR%\

echo Copying documentation...

REM Copy README and networking documentation
if exist vps-deploy\README.md copy vps-deploy\README.md %RELEASE_DIR%\
if exist vps-deploy\NETWORKING.md copy vps-deploy\NETWORKING.md %RELEASE_DIR%\

echo.
echo ========================================
echo Release package created successfully!
echo ========================================
echo.
echo Location: %RELEASE_DIR%\
echo.
echo Contents:
echo - bin\: Executable files
echo - assets\: Game assets (required)
echo - userdata\: User data directory
echo - config\: Configuration templates
echo - secrets\: For private keys (create your own)
echo - Startup scripts for each service
echo.
echo Before running:
echo 1. Copy config\.env.example to .env and configure
echo 2. Copy config\auth.env.example to auth.env and configure
echo 3. Copy config\marketplace.env.example to marketplace.env and configure
echo 4. Server settings are pre-configured for VPS at 52.247.50.106
echo 5. Ensure ports 14004, 14005, 14006 are forwarded on your VPS
echo.
echo Current VPS Configuration:
echo - Server IP: 52.247.50.106
echo - Auth Server: http://52.247.50.106:19253
echo - Game Server: 0.0.0.0:14004 (all interfaces)
echo - Query Server: 0.0.0.0:14006
echo.
echo Required Ports:
echo - 14004/tcp (Game server)
echo - 14005/tcp (Game server alternative)
echo - 14006/udp (Query server)
echo - 19253/tcp (Auth server - keep private or use reverse proxy)
echo ========================================
