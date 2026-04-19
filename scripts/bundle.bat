@echo off
REM Build a distributable Freetasia bundle with ffmpeg included.
REM Run from the repository root: scripts\bundle.bat

pushd "%~dp0.."

echo ==> Building Freetasia (release)...
cargo build --release
if %ERRORLEVEL% neq 0 (
    echo Cargo build failed
    popd
    exit /b 1
)

if exist "dist\Freetasia" rmdir /s /q "dist\Freetasia"
mkdir "dist\Freetasia"

copy "target\release\freetasia.exe" "dist\Freetasia\freetasia.exe" >nul
echo     Copied freetasia.exe

if not exist "target\tmp\ffmpeg\ffmpeg.exe" (
    echo ==> ffmpeg cache not found. Run scripts\bundle.ps1 first to download ffmpeg.
    popd
    exit /b 1
)

xcopy "target\tmp\ffmpeg\*" "dist\Freetasia\" /s /e /q /y >nul
echo     Bundled ffmpeg binaries

copy "README.md" "dist\Freetasia\README.md" >nul
copy "THIRD_PARTY_LICENSES.md" "dist\Freetasia\THIRD_PARTY_LICENSES.md" >nul
if exist "LICENSE" copy "LICENSE" "dist\Freetasia\LICENSE" >nul

echo.
echo ==> Bundle ready at: dist\Freetasia
echo     Zip this folder and distribute!

popd
