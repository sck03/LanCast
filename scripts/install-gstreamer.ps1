$ErrorActionPreference = 'Stop'
$version = '1.26.10'
$packages = @{
    "gstreamer-1.0-msvc-x86_64-$version.msi" = 'a863bf3faa49e9f33bd3cc42967b473482d4dc98655ed95cba1ac59f26fb0cfb'
    "gstreamer-1.0-devel-msvc-x86_64-$version.msi" = '8487a115fea3b0b0b4c55a9a413ad1adab0fe78f6681feedc60858a48613a121'
}
$downloadDir = Join-Path $env:RUNNER_TEMP 'lancast-gstreamer'
New-Item -ItemType Directory -Force $downloadDir | Out-Null
foreach ($name in $packages.Keys) {
    $file = Join-Path $downloadDir $name
    Invoke-WebRequest "https://gstreamer.freedesktop.org/data/pkg/windows/$version/msvc/$name" -OutFile $file
    if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant() -ne $packages[$name]) { throw "Hash mismatch: $name" }
    $process = Start-Process msiexec.exe -ArgumentList @('/i', $file, '/qn', '/norestart', 'ADDLOCAL=ALL', 'INSTALLDIR=C:\gstreamer\1.0\msvc_x86_64') -Wait -PassThru -WindowStyle Hidden
    if ($process.ExitCode -notin @(0,3010)) { throw "GStreamer install failed: $($process.ExitCode)" }
}
