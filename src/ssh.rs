//! `ssh` sin argumentos: tus servidores en una tabla (con si responden o no) para conectar,
//! subir/bajar archivos, editarlos o borrarlos, y un asistente para agregar uno nuevo.
//! Se guardan en ~/.ssh/config (el archivo estándar), así también funcionan fuera de fysh.

use crate::ask::{ask, confirm};
use crate::picker;
use crate::table::{self, Align};
use crate::{home, run_external, GRAY, GREEN, RED, RESET};
use std::io::IsTerminal;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Server {
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub identity: String,
}

impl Server {
    /// "usuario@host:puerto"
    fn target(&self) -> String {
        let user = if self.user.is_empty() { String::new() } else { format!("{}@", self.user) };
        format!("{user}{}:{}", self.host, self.port)
    }
}

fn config_path() -> Option<PathBuf> {
    home().map(|h| h.join(".ssh").join("config"))
}

/// "Clave valor" o "Clave=valor" → (clave en minúsculas, valor). None para líneas vacías o comentarios.
fn key_value(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let split = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let value = line[split..].trim_start_matches(|c: char| c.is_whitespace() || c == '=');
    Some((line[..split].to_lowercase(), value.trim().trim_matches('"').to_string()))
}

/// Servidores definidos en el config. Los bloques con comodines (Host *) son reglas, no servidores.
fn parse(text: &str) -> Vec<Server> {
    let mut servers: Vec<Server> = Vec::new();
    let mut current: Option<Server> = None;
    for line in text.lines() {
        let Some((key, value)) = key_value(line) else { continue };
        match key.as_str() {
            "host" | "match" => {
                servers.extend(current.take());
                let name = value.split_whitespace().next().unwrap_or("");
                if key == "host" && !name.contains(['*', '?', '!']) {
                    current = Some(Server { name: name.into(), host: name.into(), port: 22, ..Default::default() });
                }
            }
            "hostname" => current.iter_mut().for_each(|s| s.host = value.clone()),
            "user" => current.iter_mut().for_each(|s| s.user = value.clone()),
            "port" => current.iter_mut().for_each(|s| s.port = value.parse().unwrap_or(22)),
            "identityfile" => current.iter_mut().for_each(|s| s.identity = value.clone()),
            _ => {}
        }
    }
    servers.extend(current);
    servers
}

fn block(s: &Server) -> String {
    let mut out = format!("Host {}\n    HostName {}\n", s.name, s.host);
    if !s.user.is_empty() {
        out += &format!("    User {}\n", s.user);
    }
    out += &format!("    Port {}\n", s.port);
    if !s.identity.is_empty() {
        out += &format!("    IdentityFile {}\n", s.identity);
    }
    out
}

/// Reemplaza el bloque `Host old_name` por `server` (o lo borra si es None, o lo agrega al final
/// si no existía). Las demás líneas del archivo quedan igual.
fn update_config(text: &str, old_name: Option<&str>, server: Option<&Server>) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let is_block_start = |l: &str| key_value(l).is_some_and(|(k, _)| k == "host" || k == "match");
    let start = old_name.and_then(|name| {
        lines.iter().position(|l| {
            key_value(l).is_some_and(|(k, v)| k == "host" && v.split_whitespace().next() == Some(name))
        })
    });
    let new_block = server.map(block).unwrap_or_default();
    match start {
        Some(start) => {
            let end = lines[start + 1..].iter().position(|l| is_block_start(l)).map_or(lines.len(), |p| start + 1 + p);
            let mut out: Vec<String> = lines[..start].iter().map(|l| l.to_string()).collect();
            if !new_block.is_empty() {
                out.extend(new_block.lines().map(str::to_owned));
                out.push(String::new());
            }
            out.extend(lines[end..].iter().map(|l| l.to_string()));
            // Sin líneas en blanco de más al final.
            while out.last().is_some_and(|l| l.trim().is_empty()) {
                out.pop();
            }
            if out.is_empty() { String::new() } else { out.join("\n") + "\n" }
        }
        None if !new_block.is_empty() => {
            let base = text.trim_end();
            if base.is_empty() { new_block } else { format!("{base}\n\n{new_block}") }
        }
        None => text.to_string(),
    }
}

fn load() -> Vec<Server> {
    config_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse(&t)).unwrap_or_default()
}

/// Nombres de servidores, para el autocompletado.
pub fn host_names() -> Vec<String> {
    load().into_iter().map(|s| s.name).collect()
}

/// Guarda el cambio en ~/.ssh/config, dejando antes una copia en ~/.ssh/config.fysh-respaldo.
fn save(old_name: Option<&str>, server: Option<&Server>) -> std::io::Result<()> {
    let path = config_path().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "sin carpeta de usuario"))?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    if path.exists() {
        std::fs::copy(&path, path.with_file_name("config.fysh-respaldo"))?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, update_config(&text, old_name, server))
}

/// ¿Responde el puerto? Se revisan todos a la vez, con 1.5 s como máximo.
fn reachable(servers: &[Server]) -> Vec<bool> {
    let handles: Vec<_> = servers
        .iter()
        .map(|s| {
            let addr = format!("{}:{}", s.host, s.port);
            std::thread::spawn(move || {
                addr.to_socket_addrs()
                    .ok()
                    .and_then(|mut a| a.next())
                    .is_some_and(|a| TcpStream::connect_timeout(&a, Duration::from_millis(1500)).is_ok())
            })
        })
        .collect();
    handles.into_iter().map(|h| h.join().unwrap_or(false)).collect()
}

/// `ssh` sin argumentos.
pub fn ssh_cmd() -> i32 {
    let servers = load();
    let online = reachable(&servers);
    let rows: Vec<Vec<String>> = servers
        .iter()
        .zip(&online)
        .map(|(s, &up)| {
            let state = if up { format!("{GREEN}● responde{RESET}") } else { format!("{RED}● sin respuesta{RESET}") };
            vec![s.name.clone(), s.target(), state]
        })
        .collect();

    if !std::io::stdin().is_terminal() {
        table::print(&["Servidor", "Destino", "Estado"], &[Align::Left; 3], &rows);
        return 0;
    }
    let mut options = vec![vec!["+ Nuevo servidor".to_string(), String::new(), String::new()]];
    options.extend(rows);
    match picker::select_one(&["Servidor", "Destino", "Estado"], &options) {
        Some(0) => edit(None),
        Some(i) => server_menu(&servers[i - 1]),
        None => 0,
    }
}

fn server_menu(s: &Server) -> i32 {
    let actions = ["Conectar", "Subir archivos", "Bajar archivos", "Editar", "Borrar"];
    let rows: Vec<Vec<String>> = actions.iter().map(|a| vec![a.to_string()]).collect();
    println!("{GRAY}{} → {}{RESET}", s.name, s.target());
    match picker::select_one(&["¿Qué quieres hacer?"], &rows) {
        Some(0) => run_external(&["ssh".into(), s.name.clone()]),
        Some(1) => upload(s),
        Some(2) => download(s),
        Some(3) => edit(Some(s)),
        Some(4) => {
            if !confirm(&format!("¿Borrar {} de ~/.ssh/config? (queda copia en config.fysh-respaldo)", s.name)) {
                return 0;
            }
            report(save(Some(&s.name), None), &format!("{} borrado", s.name))
        }
        _ => 0,
    }
}

fn report(result: std::io::Result<()>, ok: &str) -> i32 {
    match result {
        Ok(()) => {
            println!("{GREEN}✓{RESET} {ok}");
            0
        }
        Err(e) => {
            eprintln!("ssh: no se pudo guardar ~/.ssh/config: {e}");
            1
        }
    }
}

/// Asistente para agregar (None) o editar un servidor. Enter acepta el valor que aparece escrito.
fn edit(existing: Option<&Server>) -> i32 {
    let base = existing.cloned().unwrap_or(Server { port: 22, ..Default::default() });
    println!("{GRAY}Escribe cada dato y presiona Enter (Ctrl+C cancela).{RESET}");
    let Some(name) = ask("Nombre corto (ej: produccion): ", &base.name) else { return 0 };
    if name.contains(char::is_whitespace) || name.contains(['*', '?']) {
        eprintln!("El nombre no puede tener espacios ni * ?");
        return 1;
    }
    if existing.map_or(true, |e| e.name != name) && load().iter().any(|s| s.name == name) {
        eprintln!("Ya hay un servidor llamado {name}.");
        return 1;
    }
    let Some(host) = ask("Host (IP o dominio): ", if base.host == base.name { "" } else { &base.host }) else { return 0 };
    let user = ask("Usuario (vacío = el de tu PC): ", &base.user).unwrap_or_default();
    let port_text = ask("Puerto: ", &base.port.to_string()).unwrap_or_else(|| "22".into());
    let Ok(port) = port_text.parse::<u16>() else {
        eprintln!("Puerto no válido: {port_text}");
        return 1;
    };
    let identity = ask("Llave privada (vacío = usar contraseña): ", &base.identity).unwrap_or_default();

    let server = Server { name: name.clone(), host, user, port, identity };
    let verb = if existing.is_some() { "actualizado" } else { "guardado" };
    let code = report(save(existing.map(|e| e.name.as_str()), Some(&server)), &format!("{name} {verb} → {}", server.target()));
    if code == 0 {
        println!("{GRAY}Conéctate con ssh (selector) o directo: ssh {name}{RESET}");
    }
    code
}

/// Subir: archivo o carpeta local → ruta en el servidor.
fn upload(s: &Server) -> i32 {
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let Some(local) = ask("Archivo o carpeta a subir: ", &format!("{cwd}{}", std::path::MAIN_SEPARATOR)) else { return 0 };
    if !std::path::Path::new(&local).exists() {
        eprintln!("No existe: {local}");
        return 1;
    }
    let Some(remote) = ask(&format!("Destino en {}: ", s.name), "~/") else { return 0 };
    run_external(&["scp".into(), "-r".into(), local, format!("{}:{remote}", s.name)])
}

/// Bajar: ruta en el servidor → carpeta local.
fn download(s: &Server) -> i32 {
    let Some(remote) = ask(&format!("Archivo o carpeta en {} a bajar: ", s.name), "~/") else { return 0 };
    let Some(local) = ask("Guardar en: ", ".") else { return 0 };
    run_external(&["scp".into(), "-r".into(), format!("{}:{remote}", s.name), local])
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "# mis servidores\nHost *\n    ServerAliveInterval 60\n\nHost prod\n    HostName 10.0.0.5\n    User root\n    Port 2222\n    IdentityFile ~/.ssh/id_ed25519\n\nHost dev web\n  hostname=dev.example.com\n";

    #[test]
    fn lee_servidores_del_config() {
        let s = parse(CONFIG);
        assert_eq!(s.len(), 2, "Host * es una regla, no un servidor");
        assert_eq!(s[0], Server { name: "prod".into(), host: "10.0.0.5".into(), user: "root".into(), port: 2222, identity: "~/.ssh/id_ed25519".into() });
        assert_eq!((s[1].name.as_str(), s[1].host.as_str(), s[1].port), ("dev", "dev.example.com", 22));
        assert_eq!(s[0].target(), "root@10.0.0.5:2222");
        assert_eq!(s[1].target(), "dev.example.com:22");
    }

    #[test]
    fn agrega_edita_y_borra_sin_tocar_lo_demas() {
        let nuevo = Server { name: "qa".into(), host: "qa.local".into(), user: "dary".into(), port: 22, identity: String::new() };
        let added = update_config(CONFIG, None, Some(&nuevo));
        assert!(added.starts_with(CONFIG.trim_end()));
        assert!(added.ends_with("Host qa\n    HostName qa.local\n    User dary\n    Port 22\n"));

        let mut edited = parse(&added)[0].clone();
        edited.port = 22;
        let text = update_config(&added, Some("prod"), Some(&edited));
        assert_eq!(parse(&text).iter().find(|s| s.name == "prod").unwrap().port, 22);
        assert!(text.contains("ServerAliveInterval 60"), "las reglas generales se conservan");

        let removed = update_config(&text, Some("prod"), None);
        let names: Vec<String> = parse(&removed).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["dev", "qa"]);
        assert!(removed.contains("# mis servidores"));
    }

    #[test]
    fn config_vacio() {
        let s = Server { name: "a".into(), host: "1.2.3.4".into(), port: 22, ..Default::default() };
        assert_eq!(update_config("", None, Some(&s)), "Host a\n    HostName 1.2.3.4\n    Port 22\n");
        assert_eq!(update_config("Host a\n    HostName 1.2.3.4\n", Some("a"), None), "");
    }
}
