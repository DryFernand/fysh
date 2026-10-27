# Instala fysh en Windows (menú Inicio + escritorio). Para instalar o actualizar:
#   irm https://raw.githubusercontent.com/DryFernand/fysh/main/install.ps1 | iex
# Si FYSH_EXE está definida (lo hace dev-install.ps1), usa ese exe en vez de descargar.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'  # sin barra de progreso, descargas mucho más rápidas
$repo = 'DryFernand/fysh'

if (Get-Process fysh -ErrorAction SilentlyContinue) { throw "Cierra fysh antes de instalar." }

$dest = "$env:LOCALAPPDATA\Programs\fysh"
New-Item -ItemType Directory -Force $dest | Out-Null

if ($env:FYSH_EXE) {
    Copy-Item $env:FYSH_EXE "$dest\fysh.exe" -Force
    Copy-Item "$PSScriptRoot\fysh.ico" "$dest\fysh.ico" -Force
} else {
    $url = "https://github.com/$repo/releases/latest/download/fysh-windows-x86_64.exe"
    Write-Host "Descargando $url"
    Invoke-WebRequest $url -OutFile "$dest\fysh.exe" -UseBasicParsing
    Invoke-WebRequest "https://raw.githubusercontent.com/$repo/main/fysh.ico" -OutFile "$dest\fysh.ico" -UseBasicParsing
}

# --- Fuente JetBrains Mono (solo para este usuario, sin admin) ---
$fontDir = "$env:LOCALAPPDATA\Microsoft\Windows\Fonts"
$hasFont = (Test-Path "$env:WINDIR\Fonts\JetBrainsMono-Regular.ttf") -or (Test-Path "$fontDir\JetBrainsMono-Regular.ttf")
if (-not $hasFont) {
    Write-Host "Instalando fuente JetBrains Mono"
    $zip = "$env:TEMP\JetBrainsMono.zip"
    $tmp = "$env:TEMP\JetBrainsMono"
    Invoke-WebRequest 'https://github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip' -OutFile $zip -UseBasicParsing
    Expand-Archive $zip $tmp -Force
    New-Item -ItemType Directory -Force $fontDir | Out-Null
    $reg = 'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\Fonts'
    foreach ($style in 'Regular', 'Bold', 'Italic', 'BoldItalic') {
        $file = "JetBrainsMono-$style.ttf"
        Copy-Item "$tmp\fonts\ttf\$file" "$fontDir\$file" -Force
        Set-ItemProperty $reg "JetBrains Mono $style (TrueType)" "$fontDir\$file"
    }
    Remove-Item $zip, $tmp -Recurse -Force
}

# --- Perfil de Windows Terminal: fuente, colores e ícono de fysh ---
$wt = Get-Command wt.exe -ErrorAction SilentlyContinue
if ($wt) {
    $fragment = @{
        profiles = @(@{
            name              = 'fysh'
            commandline       = "$dest\fysh.exe"
            icon              = "$dest\fysh.ico"
            startingDirectory = '%USERPROFILE%'
            colorScheme       = 'fysh'
            font              = @{ face = 'JetBrains Mono'; size = 11 }
            cursorShape       = 'bar'
            padding           = '12'
        })
        schemes  = @(@{
            name                = 'fysh'
            background          = '#181C24'
            foreground          = '#E6E6E6'
            cursorColor         = '#FF4646'
            selectionBackground = '#5A2A2A'
            black  = '#2A2F3A'; brightBlack  = '#5C6370'
            red    = '#FF4646'; brightRed    = '#FF7070'
            green  = '#50FA7B'; brightGreen  = '#7CFC9A'
            yellow = '#F1C40F'; brightYellow = '#FFD95A'
            blue   = '#61AFEF'; brightBlue   = '#8CC8FF'
            purple = '#C678DD'; brightPurple = '#DDA0F0'
            cyan   = '#56B6C2'; brightCyan   = '#7FD8E2'
            white  = '#D0D0D0'; brightWhite  = '#FFFFFF'
        })
    }
    $fragDir = "$env:LOCALAPPDATA\Microsoft\Windows Terminal\Fragments\fysh"
    New-Item -ItemType Directory -Force $fragDir | Out-Null
    # UTF-8 sin BOM: Set-Content -Encoding UTF8 en PowerShell 5.1 agrega BOM y puede romper el JSON
    [IO.File]::WriteAllText("$fragDir\fysh.json", ($fragment | ConvertTo-Json -Depth 5))
}

# --- Accesos directos: abren el perfil de Windows Terminal si existe, si no el exe directo ---
$wsh = New-Object -ComObject WScript.Shell
foreach ($dir in [Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('Desktop')) {
    $lnk = $wsh.CreateShortcut("$dir\fysh.lnk")
    if ($wt) {
        $lnk.TargetPath = $wt.Source
        $lnk.Arguments = '-p fysh'
    } else {
        $lnk.TargetPath = "$dest\fysh.exe"
        $lnk.Arguments = ''
    }
    $lnk.WorkingDirectory = $env:USERPROFILE  # arranca en tu carpeta de usuario, como cualquier terminal
    $lnk.IconLocation = "$dest\fysh.exe,0"
    $lnk.Description = 'fysh'
    $lnk.Save()
}
Write-Host "fysh instalada en $dest (menu Inicio y escritorio)"
if ($wt) { Write-Host "Si Windows Terminal estaba abierto, cierralo para que cargue la fuente y el perfil." }
