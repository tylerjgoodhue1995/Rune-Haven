@echo off
REM Build script for Windows release builds with assets bundled

echo Building Veloren release binaries...

REM Set environment variables for release builds
set VELOREN_USERDATA_STRATEGY=executable
set VELOREN_GIT_VERSION=/0/0

REM Build server-cli
echo Building server-cli...
cargo build --release -p veloren-server-cli

REM Build voxygen (client)
echo Building voxygen...
cargo build --release -p veloren-voxygen

REM Build auth service
echo Building auth service...
cargo build --release --manifest-path auth-service\Cargo.toml --bin veloren-beta-auth

REM Build marketplace service
echo Building marketplace service...
cargo build --release -p veloren-marketplace-service

echo.
echo Copying assets to release directory...

REM Create release directories
if not exist "target\release\assets" mkdir "target\release\assets"
if not exist "target\release\userdata" mkdir "target\release\userdata"

REM Copy assets folder
xcopy /E /I /Y "assets" "target\release\assets"

echo.
echo Build complete!
echo Binaries are in target\release\
echo Assets are in target\release\assets\
echo.
echo To package for distribution, copy the following files:
echo - target\release\veloren-server-cli.exe
echo - target\release\veloren-voxygen.exe  
echo - target\release\veloren-beta-auth.exe
echo - target\release\veloren-marketplace-service.exe
echo - target\release\assets\ (entire folder)
