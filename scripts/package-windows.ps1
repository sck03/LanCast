param([ValidateSet('Debug', 'Release')][string]$Configuration = 'Release')
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Split-Path $PSScriptRoot -Parent)).Path
Set-Location -LiteralPath $root
$dist = [IO.Path]::GetFullPath((Join-Path $root 'dist'))
$delivery = Join-Path $dist 'LanCast-Windows-x64'
$destination = Join-Path $dist ('LanCast-Windows-x64-stage-' + [guid]::NewGuid().ToString('N'))
function Assert-PackagePath([string]$Path) {
    $absolute = [IO.Path]::GetFullPath($Path)
    if ([IO.Path]::GetDirectoryName($absolute) -ne $dist -or [IO.Path]::GetFileName($absolute) -notmatch '^LanCast-Windows-x64(?:-stage-[a-f0-9]{32})?$') {
        throw "Invalid generated package path: $absolute"
    }
    $current = $absolute
    while ($current -and $current -ne $root) {
        if ((Test-Path -LiteralPath $current) -and ((Get-Item -LiteralPath $current -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Package path contains a link: $current"
        }
        $current = [IO.Path]::GetDirectoryName($current)
    }
    if ($current -ne $root) { throw "Package path is outside workspace: $absolute" }
    return $absolute
}
function Remove-PackageDirectory([string]$Path) {
    $absolute = Assert-PackagePath $Path
    if (Test-Path -LiteralPath $absolute) {
        if (Get-ChildItem -LiteralPath $absolute -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }) {
            throw 'Package contains a protected link'
        }
        Remove-Item -LiteralPath $absolute -Recurse -Force
    }
}
$destination = Assert-PackagePath $destination
New-Item -ItemType Directory $destination | Out-Null
try {
Copy-Item -LiteralPath "windows/build/$Configuration/LanCast.exe","windows/build/$Configuration/lancast_core.dll","windows/build/$Configuration/lancast_rtc.dll",.cache/ts-windows/lancast_ts.dll -Destination $destination
python scripts/build_config.py --platform windows --configuration $Configuration --check-recorded
if ($LASTEXITCODE -ne 0) { throw 'Invalid Windows product version' }
Copy-Item -LiteralPath dist/reports/build-windows.json -Destination $destination
$buildReport = Get-Content -LiteralPath dist/reports/build-windows.json -Raw | ConvertFrom-Json
$versionInfo = (Get-Item -LiteralPath "$destination/LanCast.exe").VersionInfo
if ($versionInfo.ProductVersion -ne $buildReport.version -or $versionInfo.FileVersion -ne "$($buildReport.version).$($buildReport.build_number)") { throw 'Windows executable version does not match build inputs' }
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
foreach ($component in @('libjuice', 'libsrtp', 'usrsctp', 'plog')) {
    $source = Join-Path $PWD "windows/build/_deps/datachannel-src/deps/$component"
    Get-ChildItem -LiteralPath $source -File | Where-Object { $_.Name -match '^(LICENSE|COPYING|COPYRIGHT)' } | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $licenses "$component-$($_.Name).txt")
    }
}
Copy-Item -LiteralPath 'windows/build/_deps/json-src/LICENSE.MIT' -Destination (Join-Path $licenses 'json-MIT.txt')
@'
Development build: WGC capture, hardware MF H.264, WASAPI loopback, WebRTC,
DLNA synthetic probes/live TS, and original MP4 sharing.
Requires Windows 10 22H2 or Windows 11, media components and a D3D11-aware H.264 hardware encoder.
Physical-device compatibility and performance acceptance remain required.
'@ | Set-Content -LiteralPath "$destination/BUILD-STATUS.txt" -Encoding utf8
@'
LanCast 使用说明

1. 完整解压后打开 LanCast.exe。本机网络会自动识别，电视会自动搜索。
2. 电视已安装 LanCast：选择电视，输入电视上的 8 位配对码，核对显示的完整指纹，再在电视上允许连接。
   各端请同步使用当前版本（配对协议 v2）。不兼容旧软件版本，不需要手填电视地址或指纹。
3. 选择屏幕或窗口，点击“开始投屏”；也可点击“播放视频文件”选择 MP4。
   勾选声音时会分享系统声音，包括其他应用的声音。
4. 普通 DLNA 电视不需要配对码，可直接尝试播放 MP4。投屏前先“测试电视兼容性”。
5. 点击标题栏的最小化即可收到托盘并继续投屏。单击托盘图标恢复，右键可停止投屏或退出。
   关闭窗口会退出；正在分享时会提示。托盘不可用时保留窗口。

未发现电视：确认同一 Wi-Fi/有线网络、电视接收端已打开；点击刷新电视。
多网卡/VPN 环境可在高级设置切换网络。不必手写本机 IP。
配对码过期：在电视上更新邀请/配对码，然后重新输入。

需 Windows 10 22H2 或 Windows 11。请保留同目录三个 DLL。
这是开发版，尚未完成所有电视型号、实际延迟和长时间运行验收。
'@ | Set-Content -LiteralPath "$destination/使用说明.txt" -Encoding utf8
python scripts/verify-windows.py --write-hashes $destination
if ($LASTEXITCODE -ne 0) { throw 'Staged Windows package verification failed' }
$delivery = Assert-PackagePath $delivery
Remove-PackageDirectory $delivery
$destination = Assert-PackagePath $destination
Move-Item -LiteralPath $destination -Destination $delivery
} finally {
    Remove-PackageDirectory $destination
}
