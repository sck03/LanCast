$ErrorActionPreference = 'Stop'
function Run([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with $LASTEXITCODE" }
}
$root = Split-Path $PSScriptRoot -Parent
$source = Join-Path $root '.cache/mbedtls'
$prefix = Join-Path $root '.cache/mbedtls-install'
$revision = '947808ba53faf09c526f575f5635e2e86472ba5d'
if (!(Test-Path -LiteralPath $source)) {
    Run git @('clone', '--no-checkout', 'https://github.com/Mbed-TLS/mbedtls.git', $source)
}
Run git @('-C', $source, 'checkout', '--detach', $revision)
Run git @('-C', $source, 'submodule', 'update', '--init', '--recursive')
Run python @("$source/scripts/config.py", '-f', "$source/include/mbedtls/mbedtls_config.h", 'set', 'MBEDTLS_SSL_DTLS_SRTP')
Run cmake @('-S', $source, '-B', "$root/.cache/mbedtls-build", '-G', 'Visual Studio 17 2022', '-A', 'x64', '-DENABLE_PROGRAMS=OFF', '-DENABLE_TESTING=OFF', '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded', "-DCMAKE_INSTALL_PREFIX=$prefix")
Run cmake @('--build', "$root/.cache/mbedtls-build", '--config', 'Release', '--parallel', '4')
Run cmake @('--install', "$root/.cache/mbedtls-build", '--config', 'Release')
@{ dependency='mbedtls'; revision=$revision; configuration='Release'; license='Apache-2.0 OR GPL-2.0-or-later (Apache selected)' } | ConvertTo-Json | Set-Content -LiteralPath "$prefix/build-manifest.json" -Encoding utf8
