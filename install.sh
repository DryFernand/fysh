#!/bin/sh
# Instala fysh en Linux o Mac. Para instalar o actualizar:
#   curl -fsSL https://raw.githubusercontent.com/DryFernand/fysh/main/install.sh | sh
set -e
REPO="DryFernand/fysh"

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) ASSET=fysh-linux-x86_64 ;;
    Darwin-arm64) ASSET=fysh-macos-aarch64 ;;
    *) echo "fysh: plataforma no soportada: $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac

URL="https://github.com/$REPO/releases/latest/download/$ASSET"
mkdir -p "$HOME/.local/bin"
# curl si está; si no, wget (algunos servidores mínimos solo traen uno de los dos).
if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$URL" -o "$HOME/.local/bin/fysh"
else
    wget -qO "$HOME/.local/bin/fysh" "$URL"
fi
chmod +x "$HOME/.local/bin/fysh"
echo "fysh instalada en ~/.local/bin/fysh"
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "Agrega ~/.local/bin a tu PATH para usar 'fysh'." ;; esac
