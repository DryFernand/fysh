//! `docker` sin argumentos: tus contenedores en una tabla; al elegir uno, ver sus logs, entrar,
//! reiniciar, detener, iniciar o borrar. Si Docker Desktop está apagado, ofrece encenderlo.
//! Con argumentos, docker funciona como siempre.

use crate::ask::confirm;
use crate::picker;
use crate::table::{self, truncate, Align};
use crate::{run_external, GRAY, GREEN, RED, RESET, YELLOW};
use std::io::IsTerminal;
use std::process::Command;

struct Container {
    name: String,
    image: String,
    running: bool,
    status: String,
    ports: String,
}

/// Contenedores (también los detenidos). Err con el mensaje de docker si no responde.
fn containers() -> Result<Vec<Container>, String> {
    let out = Command::new("docker")
        .args(["ps", "-a", "--format", "{{json .}}"])
        .output()
        .map_err(|_| "docker no está instalado".to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|j| {
            let s = |k: &str| j[k].as_str().unwrap_or("").to_string();
            Container { name: s("Names"), image: s("Image"), running: s("State") == "running", status: s("Status"), ports: s("Ports") }
        })
        .collect())
}

/// "0.0.0.0:2222->2222/tcp, [::]:2222->2222/tcp" → "2222→2222"
fn short_ports(ports: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for p in ports.split(", ") {
        let Some((host, inner)) = p.split_once("->") else { continue };
        let host = host.rsplit(':').next().unwrap_or(host);
        let inner = inner.split('/').next().unwrap_or(inner);
        let pair = format!("{host}→{inner}");
        if !out.contains(&pair) {
            out.push(pair);
        }
    }
    out.join(", ")
}

/// Intenta encender Docker Desktop y espera hasta 90 s a que responda.
fn start_docker() -> bool {
    let started = if cfg!(windows) {
        Command::new(r"C:\Program Files\Docker\Docker\Docker Desktop.exe").spawn().is_ok()
    } else if cfg!(target_os = "macos") {
        Command::new("open").args(["-a", "Docker"]).status().is_ok_and(|s| s.success())
    } else {
        eprintln!("{GRAY}En Linux enciéndelo con: sudo systemctl start docker{RESET}");
        false
    };
    if !started {
        return false;
    }
    print!("{GRAY}Encendiendo Docker");
    for _ in 0..45 {
        std::thread::sleep(std::time::Duration::from_secs(2));
        print!(".");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        if Command::new("docker").args(["info", "--format", "{{.ServerVersion}}"]).output().is_ok_and(|o| o.status.success()) {
            println!(" listo{RESET}");
            return true;
        }
    }
    println!(" no respondió{RESET}");
    false
}

/// `docker` sin argumentos.
pub fn docker_cmd() -> i32 {
    let list = match containers() {
        Ok(l) => l,
        Err(e) if e.contains("not installed") || e.contains("no está instalado") => {
            eprintln!("docker no está instalado.");
            return 127;
        }
        Err(e) => {
            eprintln!("{RED}Docker no responde.{RESET} {GRAY}{}{RESET}", truncate(e.lines().next().unwrap_or(""), 100));
            if !std::io::stdin().is_terminal() || !confirm("¿Encender Docker Desktop?") || !start_docker() {
                return 1;
            }
            match containers() {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            }
        }
    };
    if list.is_empty() {
        println!("{GRAY}No hay contenedores. Crea uno con docker run o docker compose up.{RESET}");
        return 0;
    }
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|c| {
            let state = if c.running {
                format!("{GREEN}● {}{RESET}", c.status)
            } else if c.status.starts_with("Restarting") {
                // Reiniciándose una y otra vez: algo le falla al arrancar (ver sus logs).
                format!("{YELLOW}↻ {}{RESET}", c.status)
            } else {
                format!("{GRAY}○ {}{RESET}", c.status)
            };
            vec![c.name.clone(), truncate(&c.image, 40), state, format!("{YELLOW}{}{RESET}", short_ports(&c.ports))]
        })
        .collect();
    let headers = ["Contenedor", "Imagen", "Estado", "Puertos"];
    if !std::io::stdin().is_terminal() {
        table::print(&headers, &[Align::Left; 4], &rows);
        return 0;
    }
    let Some(i) = picker::select_one(&headers, &rows) else { return 0 };
    container_menu(&list[i])
}

fn container_menu(c: &Container) -> i32 {
    let actions: Vec<&str> = if c.running {
        vec!["Ver logs en vivo (Ctrl+C para salir)", "Entrar al contenedor", "Reiniciar", "Detener", "Borrar"]
    } else {
        vec!["Iniciar", "Ver logs", "Borrar"]
    };
    let rows: Vec<Vec<String>> = actions.iter().map(|a| vec![a.to_string()]).collect();
    println!("{GRAY}{} ({}){RESET}", c.name, c.image);
    let Some(i) = picker::select_one(&["¿Qué quieres hacer?"], &rows) else { return 0 };
    let docker = |args: &[&str]| run_external(&std::iter::once("docker").chain(args.iter().copied()).map(String::from).collect::<Vec<_>>());
    let name = c.name.as_str();
    match actions[i] {
        a if a.starts_with("Ver logs en vivo") => docker(&["logs", "-f", "--tail", "100", name]),
        "Ver logs" => docker(&["logs", "--tail", "200", name]),
        // bash si el contenedor lo tiene; si no, sh (imágenes mínimas como Alpine).
        "Entrar al contenedor" => docker(&["exec", "-it", name, "sh", "-c", "command -v bash >/dev/null && exec bash || exec sh"]),
        "Reiniciar" => docker(&["restart", name]),
        "Detener" => docker(&["stop", name]),
        "Iniciar" => docker(&["start", name]),
        "Borrar" => {
            if !confirm(&format!("¿Borrar el contenedor {name}? Se pierde lo que tenga adentro que no esté en un volumen.")) {
                return 0;
            }
            docker(&["rm", "-f", name])
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_los_puertos() {
        assert_eq!(short_ports("0.0.0.0:2222->2222/tcp, [::]:2222->2222/tcp"), "2222→2222");
        assert_eq!(short_ports("0.0.0.0:8000->80/tcp, 0.0.0.0:443->443/tcp"), "8000→80, 443→443");
        assert_eq!(short_ports("3306/tcp"), "");
        assert_eq!(short_ports(""), "");
    }
}
