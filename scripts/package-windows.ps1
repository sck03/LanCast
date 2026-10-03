$ErrorActionPreference = 'Stop'
$gst = 'C:\gstreamer\1.0\msvc_x86_64'
$destination = Join-Path $PWD 'dist/LanCast-Windows-x64'
New-Item -ItemType Directory -Force $destination | Out-Null
Copy-Item windows/build/Release/LanCast.exe,windows/build/Release/lancast_core.dll -Destination $destination
# Dynamic LGPL libraries remain replaceable by users; do not statically merge them.
Copy-Item "$gst/bin/*.dll" -Destination $destination
New-Item -ItemType Directory -Force "$destination/lib","$destination/share" | Out-Null
Copy-Item "$gst/lib/gstreamer-1.0" -Destination "$destination/lib" -Recurse
Copy-Item "$gst/libexec" -Destination $destination -Recurse
if (Test-Path "$gst/share/licenses") { Copy-Item "$gst/share/licenses" -Destination "$destination/share" -Recurse }
Copy-Item LICENSE,THIRD_PARTY.md -Destination $destination
Copy-Item docs -Destination $destination -Recurse
