# Instrucciones para agentes

fysh es una shell propia escrita en Rust (Windows, Linux y Mac).

## Publicar una versión nueva

1. Cambia `version` en `Cargo.toml` (por ejemplo `0.2.0`) y corre `cargo build` para que se actualice `Cargo.lock`.
2. Verifica con `cargo test`, luego haz commit y push a `main`. Espera a que el workflow `ci`
   (tests en Windows, Linux y Mac) termine en verde antes de crear el tag: los tests con rutas
   de Windows (`C:\...`) fallan en Linux/Mac, márcalos con `#[cfg(windows)]` o arma las rutas con `join`.
3. Crea el tag y súbelo; el número debe coincidir con el de `Cargo.toml`:
   ```
   git tag -a v0.2.0 -m "fysh v0.2.0"
   git push origin v0.2.0
   ```

El tag dispara `.github/workflows/release.yml`, que compila para Windows, Linux y Mac y publica en GitHub Releases:
`fysh-windows-x86_64.exe`, `fysh-linux-x86_64`, `fysh-macos-aarch64`.
`install.ps1` e `install.sh` descargan esos nombres exactos: si cambias uno, cambia los tres archivos.

Revisa que el workflow termine en verde antes de dar la versión por publicada.
