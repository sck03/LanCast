$ErrorActionPreference = 'Stop'
$destination = Join-Path $PWD 'dist/LanCast-Windows-x64'
New-Item -ItemType Directory -Force $destination | Out-Null
Copy-Item -LiteralPath windows/build/Release/LanCast.exe,windows/build/Release/lancast_core.dll,windows/build/Release/lancast_rtc.dll,.cache/ts-windows/lancast_ts.dll -Destination $destination
Copy-Item -LiteralPath LICENSE,THIRD_PARTY.md -Destination $destination
$licenses = Join-Path $destination 'licenses'
New-Item -ItemType Directory -Force $licenses | Out-Null
$notices = @{
    'FFmpeg-LGPL-2.1.txt' = '.cache/ffmpeg-8.0.1/COPYING.LGPLv2.1'
    'libdatachannel-MPL-2.0.txt' = 'windows/build/_deps/datachannel-src/LICENSE'
    'opus-COPYING.txt' = 'windows/build/_deps/opus-src/COPYING'
    'mbedtls-LICENSE.txt' = '.cache/mbedtls/LICENSE'
}
foreach ($name in $notices.Keys) { Copy-Item -LiteralPath $notices[$name] -Destination (Join-Path $licenses $name) }
@'
Development build: WGC capture, hardware MF H.264, WASAPI loopback, WebRTC,
DLNA synthetic probes/live TS, and original MP4 sharing.
Requires Windows 10 22H2 or Windows 11, media components and a D3D11-aware H.264 hardware encoder.
Physical-device compatibility and performance acceptance remain required.
'@ | Set-Content -LiteralPath "$destination/BUILD-STATUS.txt" -Encoding utf8
Get-ChildItem -LiteralPath $destination -File | Get-FileHash -Algorithm SHA256 | Select-Object Hash,Path | ConvertTo-Json | Set-Content -LiteralPath "$destination/SHA256.json"
