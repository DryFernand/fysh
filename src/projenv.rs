//! Entorno por proyecto. Al entrar a la carpeta de un proyecto (o a una de sus subcarpetas):
//! - activa su entorno virtual de Python (.venv / venv): `python` y `pip` pasan a ser los del proyecto;
//! - carga las variables de su `.env` en esta sesión. La primera vez pregunta (como `direnv allow`),
//!   para no cargar variables de repos que no conoces, y recuerda la respuesta.
//! Al salir del proyecto todo vuelve a como estaba. El prompt muestra la versión de Python/Node.

use crate::picker;
use crate::{home, GRAY, GREEN, RESET};
use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct Active {
    root: PathBuf,
    /// Valor anterior de cada variable que se cambió, para devolverlo al salir.
    saved: Vec<(String, Option<OsString>)>,
    python: Option<String>,
}

static STATE: Mutex<Option<Active>> = Mutex::new(None);

/// El entorno virtual de una carpeta: (carpeta del venv, carpeta con python/pip).
fn venv_in(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    for name in [".venv", "venv"] {
        let venv = dir.join(name);
        let bin = if cfg!(windows) { venv.join("Scripts") } else { venv.join("bin") };
        let python = bin.join(if cfg!(windows) { "python.exe" } else { "python" });
        if python.is_file() {
            return Some((venv, bin));
        }
    }
    None
}

/// Raíz del proyecto: la carpeta (esta o una de arriba) con `.env` o entorno virtual.
/// La carpeta de usuario no cuenta como proyecto.
fn find_root(cwd: &Path) -> Option<PathBuf> {
    let home = home();
    cwd.ancestors()
        .take_while(|d| home.as_deref() != Some(*d))
        .find(|d| d.join(".env").is_file() || venv_in(d).is_some())
        .map(Path::to_path_buf)
}

/// "KEY=valor" por línea; acepta `export`, comillas y comentarios. No expande variables.
fn parse_dotenv(text: &str) -> Vec<(String, String)> {
    let mut vars = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            continue;
        }
        let value = value.trim();
        let value = if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
            inner.replace("\\n", "\n")
        } else if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            inner.to_string()
        } else {
            // Sin comillas: lo que va después de " #" es un comentario.
            value.split(" #").next().unwrap_or("").trim().to_string()
        };
        vars.push((key.to_string(), value));
    }
    vars
}

fn decisions_file() -> Option<PathBuf> {
    home().map(|h| h.join(".fysh").join("env-permitidos"))
}

/// Respuesta guardada para el .env de esta carpeta: Some(true) siempre, Some(false) nunca.
fn saved_decision(root: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(decisions_file()?).ok()?;
    let key = root.display().to_string();
    text.lines().rev().find_map(|l| {
        let (answer, path) = l.split_once('\t')?;
        (path == key).then_some(answer == "si")
    })
}

fn save_decision(root: &Path, allow: bool) {
    let Some(file) = decisions_file() else { return };
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    text += &format!("{}\t{}\n", if allow { "si" } else { "no" }, root.display());
    let _ = file.parent().map(std::fs::create_dir_all);
    let _ = std::fs::write(file, text);
}

/// ¿Cargar el .env? Pregunta la primera vez (si hay teclado) y recuerda "siempre" / "nunca".
fn allow_dotenv(root: &Path) -> bool {
    if let Some(answer) = saved_decision(root) {
        return answer;
    }
    if !std::io::stdin().is_terminal() {
        return false;
    }
    let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    println!("{GRAY}{name} tiene un archivo .env con variables (contraseñas, URLs de bases de datos…).{RESET}");
    let rows = vec![
        vec!["Cargarlas siempre en esta carpeta".to_string()],
        vec!["Cargarlas solo esta vez".to_string()],
        vec!["No cargarlas (no volver a preguntar)".to_string()],
    ];
    match picker::select_one(&["¿Cargar el .env en esta sesión?"], &rows) {
        Some(0) => {
            save_decision(root, true);
            true
        }
        Some(1) => true,
        Some(2) => {
            save_decision(root, false);
            false
        }
        _ => false,
    }
}

fn set(saved: &mut Vec<(String, Option<OsString>)>, key: &str, value: impl AsRef<std::ffi::OsStr>) {
    if !saved.iter().any(|(k, _)| k == key) {
        saved.push((key.to_string(), std::env::var_os(key)));
    }
    std::env::set_var(key, value);
}

/// Versión de Python del entorno virtual, de su pyvenv.cfg ("version = 3.12.4" → "3.12").
fn venv_python_version(venv: &Path) -> Option<String> {
    let text = std::fs::read_to_string(venv.join("pyvenv.cfg")).ok()?;
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        matches!(k.trim(), "version" | "version_info").then(|| v.trim().split('.').take(2).collect::<Vec<_>>().join("."))
    })
}

fn activate(root: &Path) -> Active {
    let mut saved = Vec::new();
    let mut python = None;
    let mut notes = Vec::new();
    if let Some((venv, bin)) = venv_in(root) {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let joined = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path))).unwrap_or(path);
        set(&mut saved, "PATH", joined);
        set(&mut saved, "VIRTUAL_ENV", &venv);
        python = venv_python_version(&venv);
        notes.push(format!("entorno {} activado", venv.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
    }
    let dotenv = root.join(".env");
    if dotenv.is_file() && allow_dotenv(root) {
        let vars = std::fs::read_to_string(&dotenv).map(|t| parse_dotenv(&t)).unwrap_or_default();
        for (k, v) in &vars {
            set(&mut saved, k, v);
        }
        notes.push(format!("{} variables de .env cargadas", vars.len()));
    }
    if !notes.is_empty() {
        println!("{GREEN}✓{RESET} {GRAY}{}{RESET}", notes.join(" · "));
    }
    Active { root: root.to_path_buf(), saved, python }
}

fn deactivate(active: Active) {
    for (key, old) in active.saved.into_iter().rev() {
        match old {
            Some(v) => std::env::set_var(&key, v),
            None => std::env::remove_var(&key),
        }
    }
}

/// Llamar cada vez que cambia la carpeta actual.
pub fn on_dir_change(cwd: &Path) {
    let root = find_root(cwd);
    let Ok(mut state) = STATE.lock() else { return };
    if state.as_ref().map(|a| &a.root) == root.as_ref() {
        return;
    }
    if let Some(old) = state.take() {
        deactivate(old);
    }
    *state = root.map(|r| activate(&r));
}

/// Versión de Node (una sola consulta por sesión).
fn node_version() -> Option<&'static str> {
    static NODE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    NODE.get_or_init(|| {
        let program = crate::which("node")?;
        let out = std::process::Command::new(program).arg("-v").output().ok()?;
        let v = String::from_utf8_lossy(&out.stdout).trim().trim_start_matches('v').to_string();
        Some(v.split('.').take(2).collect::<Vec<_>>().join("."))
    })
    .as_deref()
}

/// Segmento del prompt: " py 3.12" si hay un entorno de Python activo, " node 20.11" en carpetas con package.json.
pub fn prompt_segment() -> String {
    let mut out = String::new();
    if let Some(py) = STATE.lock().ok().and_then(|s| s.as_ref().and_then(|a| a.python.clone())) {
        out += &format!(" {GREEN}py {py}{RESET}");
    }
    let has_package = std::env::current_dir().is_ok_and(|d| d.join("package.json").is_file());
    if has_package {
        if let Some(v) = node_version() {
            out += &format!(" {GREEN}node {v}{RESET}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_archivos_env() {
        let text = "# config\nDB_HOST=localhost\nexport DB_PORT = 3306\nNOMBRE=\"Auto Spot\"\nRAW='a $b'\nURL=http://x # comentario\nMALO\n=sin nombre\nMULTI=\"a\\nb\"\n";
        let vars = parse_dotenv(text);
        let get = |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("DB_HOST"), Some("localhost"));
        assert_eq!(get("DB_PORT"), Some("3306"));
        assert_eq!(get("NOMBRE"), Some("Auto Spot"));
        assert_eq!(get("RAW"), Some("a $b"));
        assert_eq!(get("URL"), Some("http://x"));
        assert_eq!(get("MULTI"), Some("a\nb"));
        assert_eq!(vars.len(), 6);
    }

    #[test]
    fn encuentra_la_raiz_del_proyecto() {
        let base = std::env::temp_dir().join(format!("fysh-env-{}", std::process::id()));
        let deep = base.join("backend").join("app");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(find_root(&deep), None);
        std::fs::write(base.join("backend").join(".env"), "A=1").unwrap();
        assert_eq!(find_root(&deep).as_deref(), Some(base.join("backend").as_path()));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
