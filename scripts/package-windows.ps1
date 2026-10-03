$ErrorActionPreference = 'Stop'
$destination = Join-Path $PWD 'dist/LanCast-Windows-x64'
New-Item -ItemType Directory -Force $destination | Out-Null
Copy-Item -LiteralPath windows/build/Release/LanCast.exe,windows/build/Release/lancast_core.dll -Destination $destination
Copy-Item -LiteralPath LICENSE,THIRD_PARTY.md -Destination $destination
@'
Development build: WSS pairing and file/DLNA playback control.
The custom Windows WGC/MF/libwebrtc media backend is not included.
Screen mirroring is disabled. This is not the v1.0 release.
'@ | Set-Content -LiteralPath "$destination/BUILD-STATUS.txt" -Encoding utf8
Get-ChildItem -LiteralPath $destination -File | Get-FileHash -Algorithm SHA256 | Select-Object Hash,Path | ConvertTo-Json | Set-Content -LiteralPath "$destination/SHA256.json"
