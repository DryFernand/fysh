# Para desarrollo: compila tu código local e instala ese exe (no descarga nada).
# Úsalo cada vez que cambies el código y quieras probarlo como app.

# cargo escribe su progreso en stderr; no usar ErrorActionPreference=Stop aquí
# porque PowerShell 5.1 lo tomaría como error. Se revisa el código de salida.
cargo build --release --manifest-path "$PSScriptRoot\Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw "fallo la compilacion" }
$ErrorActionPreference = 'Stop'

$env:FYSH_EXE = "$PSScriptRoot\target\release\fysh.exe"
try { & "$PSScriptRoot\install.ps1" } finally { Remove-Item Env:FYSH_EXE }
