# Compila la version de `main` en GitHub DENTRO del contenedor de pruebas (fysh-ssh, Linux)
# y la instala ahi, sin publicar ninguna version. Despues: ssh Prueba
# La primera vez instala Rust y git en el contenedor (tarda unos minutos).
$ErrorActionPreference = 'Stop'
$container = 'fysh-ssh'

if (-not (docker ps --filter "name=$container" --format '{{.Names}}')) { throw "El contenedor $container no esta encendido (enciende Docker Desktop)." }

Write-Host "Instalando herramientas de compilacion en el contenedor (solo la primera vez)..."
docker exec $container apk add --no-cache git cargo build-base | Out-Null

# El script va en un archivo: pasado como texto, PowerShell 5.1 rompe las comillas y los saltos de linea.
$build = @'
set -e
cd ~
if [ -d fysh-src ]; then cd fysh-src && git fetch -q origin && git reset -q --hard origin/main; else git clone -q https://github.com/DryFernand/fysh.git fysh-src && cd fysh-src; fi
echo "Compilando $(git log --oneline -1)..."
cargo build --release 2>&1 | tail -3
# Copiar y renombrar (no sobrescribir): funciona aunque fysh esté abierto en el contenedor.
mkdir -p ~/.local/bin && cp target/release/fysh ~/.local/bin/fysh.new && mv -f ~/.local/bin/fysh.new ~/.local/bin/fysh
~/.local/bin/fysh --version
'@
$tmp = Join-Path $env:TEMP 'fysh-build.sh'
[IO.File]::WriteAllText($tmp, $build.Replace("`r`n", "`n"))
docker cp $tmp "${container}:/tmp/fysh-build.sh" | Out-Null
Remove-Item $tmp
# La carpeta del usuario prueba es /config (docker exec no la pone sola).
docker exec -u prueba -e HOME=/config -w /config $container sh /tmp/fysh-build.sh
if ($LASTEXITCODE -ne 0) { throw "Fallo la compilacion dentro del contenedor." }
Write-Host "Listo: entra con  ssh Prueba"
