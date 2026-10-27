# Instala fysh en Windows (menú Inicio + escritorio). Para instalar o actualizar:
#   irm https://raw.githubusercontent.com/DryFernand/fysh/main/install.ps1 | iex
# Si FYSH_EXE está definida (lo hace dev-install.ps1), usa ese exe en vez de descargar.
$ErrorActionPreference = 'Stop'
$repo = 'DryFernand/fysh'

if (Get-Process fysh -ErrorAction SilentlyContinue) { throw "Cierra fysh antes de instalar." }

$dest = "$env:LOCALAPPDATA\Programs\fysh"
New-Item -ItemType Directory -Force $dest | Out-Null

if ($env:FYSH_EXE) {
    Copy-Item $env:FYSH_EXE "$dest\fysh.exe" -Force
} else {
    $url = "https://github.com/$repo/releases/latest/download/fysh-windows-x86_64.exe"
    Write-Host "Descargando $url"
    Invoke-WebRequest $url -OutFile "$dest\fysh.exe" -UseBasicParsing
}

$wsh = New-Object -ComObject WScript.Shell
foreach ($dir in [Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('Desktop')) {
    $lnk = $wsh.CreateShortcut("$dir\fysh.lnk")
    $lnk.TargetPath = "$dest\fysh.exe"
    $lnk.WorkingDirectory = $env:USERPROFILE  # arranca en tu carpeta de usuario, como cualquier terminal
    $lnk.IconLocation = "$dest\fysh.exe,0"
    $lnk.Description = 'fysh'
    $lnk.Save()
}
Write-Host "fysh instalada en $dest (menu Inicio y escritorio)"
