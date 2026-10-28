//! Aviso de versión nueva y comando `actualizar`.
//! Al abrir fysh se consulta en segundo plano (como mucho cada 6 horas) cuál es el último
//! release en GitHub y se guarda en ~/.fysh/ultima-version. El aviso usa ese dato guardado,
//! así abrir fysh nunca espera a internet.

use crate::{home, GRAY, GREEN, RED, RESET, YELLOW};
use std::path::PathBuf;
use std::process::Command;

const REPO: &str = "DryFernand/fysh";
const CURRENT: &str = env!("CARGO_PKG_VERSION");
const CHECK_EVERY: i64 = 6 * 3600;

/// ¿La versión `a` es más vieja que `b`? ("0.2.1" < "0.2.10")
pub fn older(a: &str, b: &str) -> bool {
    let nums = |v: &str| v.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    nums(a) < nums(b)
}

fn cache_file() -> Option<PathBuf> {
    home().map(|h| h.join(".fysh").join("ultima-version"))
}

/// (cuándo se consultó, versión)
fn read_cache() -> Option<(i64, String)> {
    let text = std::fs::read_to_string(cache_file()?).ok()?;
    let (when, version) = text.trim().split_once('\t')?;
    Some((when.parse().ok()?, version.to_string()))
}

fn write_cache(version: &str) {
    let Some(file) = cache_file() else { return };
    let _ = file.parent().map(std::fs::create_dir_all);
    let _ = std::fs::write(file, format!("{}\t{version}\n", chrono::Local::now().timestamp()));
}

/// ".../releases/tag/v0.2.3" → "0.2.3"
fn version_from_url(url: &str) -> Option<String> {
    let v = url.trim().rsplit_once("/tag/v")?.1;
    (v.split('.').count() == 3 && v.split('.').all(|p| p.parse::<u32>().is_ok())).then(|| v.to_string())
}

/// Último release publicado. /releases/latest redirige a /releases/tag/vX.Y.Z: basta con ver
/// a dónde redirige (sin usar la API de GitHub, que limita las consultas).
fn latest_version() -> Option<String> {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let url = format!("https://github.com/{REPO}/releases/latest");
    let out = Command::new("curl")
        .args(["-fsSIL", "--max-time", "8", "-o", null, "-w", "%{url_effective}", &url])
        .output()
        .ok()?;
    version_from_url(&String::from_utf8_lossy(&out.stdout))
}

/// Al abrir fysh: refrescar en segundo plano el dato de la última versión (si tiene más de 6 h)
/// y borrar el ejecutable viejo que deja `actualizar` en Windows.
pub fn check_in_background() {
    std::thread::spawn(|| {
        if let Some(old) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("fysh.old.exe"))) {
            let _ = std::fs::remove_file(old);
        }
        let now = chrono::Local::now().timestamp();
        if read_cache().is_some_and(|(when, _)| now - when < CHECK_EVERY) {
            return;
        }
        if let Some(v) = latest_version() {
            write_cache(&v);
        }
    });
}

/// Aviso para mostrar bajo el banner si hay una versión más nueva.
pub fn notice() -> Option<String> {
    let (_, latest) = read_cache()?;
    older(CURRENT, &latest).then(|| {
        format!("  {YELLOW}La versión {latest} de fysh está disponible{RESET} {GRAY}(tienes la {CURRENT}). Escribe{RESET} {YELLOW}actualizar{RESET} {GRAY}para instalarla.{RESET}\n")
    })
}

/// Nombre del ejecutable publicado para este sistema.
fn asset() -> Option<&'static str> {
    if cfg!(windows) {
        Some("fysh-windows-x86_64.exe")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("fysh-linux-x86_64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("fysh-macos-aarch64")
    } else {
        None
    }
}

/// `actualizar`: descarga la última versión y reemplaza este fysh. Funciona con fysh abierta:
/// en Windows el ejecutable en uso se renombra a fysh.old.exe (se borra al próximo inicio).
pub fn actualizar_cmd() -> i32 {
    println!("{GRAY}Buscando la última versión…{RESET}");
    let Some(latest) = latest_version() else {
        eprintln!("{RED}No se pudo consultar GitHub.{RESET} Revisa tu conexión a internet.");
        return 1;
    };
    write_cache(&latest);
    if !older(CURRENT, &latest) {
        println!("{GREEN}✓{RESET} Ya tienes la última versión (v{CURRENT}).");
        return 0;
    }
    let Some(asset) = asset() else {
        eprintln!("No hay versión publicada para este sistema.");
        return 1;
    };
    let Ok(exe) = std::env::current_exe() else { return 1 };
    let Some(dir) = exe.parent() else { return 1 };
    let new = dir.join(if cfg!(windows) { "fysh.new.exe" } else { "fysh.new" });

    println!("{GRAY}Descargando v{latest}…{RESET}");
    let url = format!("https://github.com/{REPO}/releases/download/v{latest}/{asset}");
    let downloaded = Command::new("curl").args(["-fsSL", "-o"]).arg(&new).arg(&url).status().is_ok_and(|s| s.success());
    if !downloaded {
        let _ = std::fs::remove_file(&new);
        eprintln!("{RED}No se pudo descargar{RESET} {url}");
        return 1;
    }

    if let Err(e) = replace(&exe, &new) {
        let _ = std::fs::remove_file(&new);
        eprintln!("{RED}No se pudo reemplazar fysh:{RESET} {e}");
        return 1;
    }
    println!("{GREEN}✓{RESET} fysh actualizada a v{latest}. {GRAY}Cierra y vuelve a abrir fysh para usarla.{RESET}");
    0
}

/// Pone `new` en lugar de `exe` (que puede estar en uso).
fn replace(exe: &std::path::Path, new: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        // Windows no deja sobrescribir un .exe en uso, pero sí renombrarlo.
        let old = exe.with_file_name("fysh.old.exe");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old)?;
        if let Err(e) = std::fs::rename(new, exe) {
            let _ = std::fs::rename(&old, exe); // dejar todo como estaba
            return Err(e);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(new, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(new, exe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_la_version_de_la_redireccion() {
        assert_eq!(version_from_url("https://github.com/DryFernand/fysh/releases/tag/v0.2.3").as_deref(), Some("0.2.3"));
        assert_eq!(version_from_url("https://github.com/DryFernand/fysh/releases"), None);
        assert_eq!(version_from_url("https://github.com/x/releases/tag/vnada"), None);
    }

    #[test]
    fn compara_versiones() {
        assert!(older("0.2.1", "0.2.2") && older("0.2.9", "0.2.10") && older("0.9.9", "1.0.0"));
        assert!(!older("0.3.0", "0.2.9") && !older("0.2.2", "0.2.2"));
    }
}
