//! `run`: lo que se puede ejecutar en el proyecto, en un selector. Busca en la carpeta actual y
//! en sus subcarpetas directas (monorepos con frontend/ y backend/):
//! scripts de package.json (npm/pnpm/yarn/bun según el lockfile), comandos de Django (manage.py,
//! con el Python del .venv), servicios de docker-compose, objetivos de Makefile y cargo.
//! `run dev` ejecuta directo; si hay varios "dev" (frontend y backend) pregunta cuál.

use crate::picker;
use crate::table::{self, truncate, Align};
use crate::{Shell, BLUE, GRAY, RESET};
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
struct Script {
    name: String,
    folder: PathBuf,
    command: String,
    about: String,
    /// npm, django, docker, make, cargo (para pasar argumentos extra bien).
    kind: &'static str,
}

/// Carpetas que nunca tienen scripts propios del proyecto.
const SKIP: &[&str] = &["node_modules", "target", "dist", "build", "venv", "__pycache__", "vendor"];

fn package_manager(dir: &Path) -> &'static str {
    if dir.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if dir.join("yarn.lock").exists() {
        "yarn"
    } else if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
        "bun"
    } else {
        "npm"
    }
}

/// Python del entorno virtual del proyecto, si tiene uno.
fn project_python(dir: &Path) -> String {
    for venv in [".venv", "venv", "env"] {
        let exe = if cfg!(windows) { dir.join(venv).join("Scripts").join("python.exe") } else { dir.join(venv).join("bin").join("python") };
        if exe.is_file() {
            return format!("\"{}\"", exe.display());
        }
    }
    if cfg!(windows) { "python".into() } else { "python3".into() }
}

/// Nombres de servicios de un docker-compose (sin librería de YAML: la sección `services:`).
fn compose_services(text: &str) -> Vec<String> {
    let mut services = Vec::new();
    let mut in_services = false;
    let mut indent = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let level = line.len() - line.trim_start().len();
        if level == 0 {
            in_services = trimmed == "services:";
            indent = None;
            continue;
        }
        if in_services && level == *indent.get_or_insert(level) {
            if let Some(name) = trimmed.strip_suffix(':') {
                services.push(name.trim_matches(['"', '\'']).to_string());
            }
        }
    }
    services
}

/// Objetivos de un Makefile ("build: dep" → "build"), sin los especiales (.PHONY) ni patrones (%).
fn make_targets(text: &str) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t', '#', '.']) {
            continue;
        }
        let Some((name, rest)) = line.split_once(':') else { continue };
        let name = name.trim();
        if rest.starts_with('=') || name.is_empty() || name.contains([' ', '%', '$', '=']) || targets.iter().any(|t| t == name) {
            continue;
        }
        targets.push(name.to_string());
    }
    targets
}

/// Lo que se puede ejecutar en una carpeta.
fn detect(dir: &Path) -> Vec<Script> {
    let mut out = Vec::new();
    let mut add = |name: &str, command: String, about: &str, kind| {
        out.push(Script { name: name.into(), folder: dir.to_path_buf(), command, about: about.into(), kind });
    };

    if let Ok(text) = fs::read_to_string(dir.join("package.json")) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            let pm = package_manager(dir);
            for (name, body) in json["scripts"].as_object().into_iter().flatten() {
                let command = match pm {
                    "yarn" => format!("yarn {name}"),
                    pm => format!("{pm} run {name}"),
                };
                add(name, command, body.as_str().unwrap_or(""), "npm");
            }
        }
    }

    if dir.join("manage.py").is_file() {
        let py = project_python(dir);
        for (name, about) in [
            ("runserver", "levantar el servidor de Django"),
            ("migrate", "aplicar migraciones"),
            ("makemigrations", "crear migraciones"),
            ("test", "correr los tests"),
            ("shell", "consola de Django"),
            ("createsuperuser", "crear un superusuario"),
        ] {
            add(name, format!("{py} manage.py {name}"), about, "django");
        }
    }

    for file in ["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"] {
        let Ok(text) = fs::read_to_string(dir.join(file)) else { continue };
        add("up", "docker compose up -d".into(), "levantar todos los servicios", "docker");
        add("down", "docker compose down".into(), "detener todos los servicios", "docker");
        for svc in compose_services(&text) {
            add(&svc, format!("docker compose up -d {svc}"), "levantar este servicio", "docker");
        }
        break;
    }

    if let Ok(text) = fs::read_to_string(dir.join("Makefile")) {
        for t in make_targets(&text) {
            add(&t, format!("make {t}"), "objetivo de Makefile", "make");
        }
    }

    if dir.join("Cargo.toml").is_file() {
        add("run", "cargo run".into(), "compilar y ejecutar", "cargo");
        add("build", "cargo build --release".into(), "compilar optimizado", "cargo");
        add("test", "cargo test".into(), "correr los tests", "cargo");
    }
    out
}

/// La carpeta actual y sus subcarpetas directas.
fn collect(cwd: &Path) -> Vec<Script> {
    let mut scripts = detect(cwd);
    let mut subdirs: Vec<PathBuf> = fs::read_dir(cwd)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .filter(|p| {
                    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                    !name.starts_with('.') && !SKIP.contains(&name.as_str())
                })
                .collect()
        })
        .unwrap_or_default();
    subdirs.sort();
    for dir in subdirs {
        scripts.extend(detect(&dir));
    }
    scripts
}

fn folder_label(folder: &Path, cwd: &Path) -> String {
    match folder.strip_prefix(cwd) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".into(),
        Ok(rel) => rel.display().to_string(),
        Err(_) => folder.display().to_string(),
    }
}

/// Ejecuta el script en su carpeta y vuelve a la carpeta donde estabas.
fn launch(shell: &mut Shell, script: &Script, extra: &[String], cwd: &Path) -> i32 {
    let mut command = script.command.clone();
    if !extra.is_empty() {
        // npm/pnpm/bun necesitan "--" para pasarle argumentos al script, no a npm.
        if script.kind == "npm" && !command.starts_with("yarn") {
            command += " --";
        }
        for a in extra {
            command += &format!(" \"{a}\"");
        }
    }
    println!("{GRAY}→ {command}  (en {}){RESET}", folder_label(&script.folder, cwd));
    if let Err(e) = std::env::set_current_dir(&script.folder) {
        eprintln!("run: {}: {e}", script.folder.display());
        return 1;
    }
    let code = crate::exec::run_line(shell, &command);
    let _ = std::env::set_current_dir(cwd);
    code
}

/// `run` (selector) o `run <nombre> [argumentos]`.
pub fn run_cmd(shell: &mut Shell, args: &[String]) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_default();
    let scripts = collect(&cwd);
    if scripts.is_empty() {
        println!("{GRAY}No encontré nada para ejecutar aquí (package.json, manage.py, docker-compose, Makefile, Cargo.toml).{RESET}");
        return 1;
    }
    let (wanted, extra) = match args.split_first() {
        Some((name, rest)) => (Some(name.as_str()), rest),
        None => (None, &[][..]),
    };
    let candidates: Vec<&Script> = scripts.iter().filter(|s| wanted.map_or(true, |w| s.name == w)).collect();
    if candidates.is_empty() {
        eprintln!("run: no hay ningún script llamado \"{}\". Escribe run para ver la lista.", wanted.unwrap_or(""));
        return 1;
    }
    if let [only] = candidates.as_slice() {
        if wanted.is_some() {
            return launch(shell, only, extra, &cwd);
        }
    }

    let rows: Vec<Vec<String>> = candidates
        .iter()
        .map(|s| {
            vec![s.name.clone(), format!("{BLUE}{}{RESET}", folder_label(&s.folder, &cwd)), format!("{GRAY}{}{RESET}", truncate(&s.about, 45))]
        })
        .collect();
    let headers = ["Script", "Carpeta", "Qué hace"];
    if !std::io::stdin().is_terminal() {
        table::print(&headers, &[Align::Left; 3], &rows);
        return 0;
    }
    match picker::select_one(&headers, &rows) {
        Some(i) => launch(shell, candidates[i], extra, &cwd),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_servicios_de_compose() {
        let yml = "version: '3'\nservices:\n  backend:\n    build: .\n    ports:\n      - \"8000:8000\"\n  \"db\":\n    image: mysql\n# comentario\nvolumes:\n  datos:\n";
        assert_eq!(compose_services(yml), ["backend", "db"]);
    }

    #[test]
    fn lee_objetivos_de_makefile() {
        let mk = ".PHONY: build\nCC := gcc\nbuild: deps\n\tgo build\ntest:\n\tgo test\n%.o: %.c\n\t$(CC)\nbuild:\n";
        assert_eq!(make_targets(mk), ["build", "test"]);
    }

    #[test]
    fn encuentra_scripts_en_subcarpetas() {
        let base = std::env::temp_dir().join(format!("fysh-run-{}", std::process::id()));
        let front = base.join("frontend");
        let back = base.join("backend");
        fs::create_dir_all(&front).unwrap();
        fs::create_dir_all(back.join("node_modules").join("x")).unwrap();
        fs::write(front.join("package.json"), r#"{"scripts":{"dev":"next dev","build":"next build"}}"#).unwrap();
        fs::write(front.join("pnpm-lock.yaml"), "").unwrap();
        fs::write(back.join("manage.py"), "").unwrap();
        fs::write(back.join("node_modules").join("x").join("package.json"), r#"{"scripts":{"no":"x"}}"#).unwrap();

        let scripts = collect(&base);
        let names: Vec<(&str, &str)> = scripts.iter().map(|s| (s.name.as_str(), s.command.as_str())).collect();
        assert!(names.contains(&("dev", "pnpm run dev")), "{names:?}");
        assert!(names.iter().any(|(n, c)| *n == "migrate" && c.ends_with("manage.py migrate")));
        assert!(!names.iter().any(|(n, _)| *n == "no"), "node_modules se ignora");
        assert_eq!(folder_label(&front, &base), "frontend");
        fs::remove_dir_all(&base).unwrap();
    }
}
