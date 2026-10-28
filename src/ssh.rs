//! `ssh` sin argumentos: tus servidores en una tabla (con si responden o no) para conectar,
//! subir/bajar archivos, editarlos o borrarlos, y un asistente para agregar uno nuevo.
//! Se guardan en ~/.ssh/config (el archivo estándar), así también funcionan fuera de fysh.
//!
//! `ssh` con argumentos corre el ssh real, pero si la llave de `-i` no existe busca tus llaves
//! privadas y te deja elegir; si está fuera de ~/.ssh ofrece copiarla allí y protegerla
//! (OpenSSH rechaza llaves que otros usuarios pueden leer); y si la conexión funcionó y el
//! servidor no estaba guardado, ofrece guardarlo.

use crate::ask::{ask, confirm};
use crate::picker;
use crate::table::{self, Align};
use crate::{home, run_external, tilde, GRAY, GREEN, RED, RESET};
use std::fs;
use std::io::{IsTerminal, Read};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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
    let actions = ["Conectar", "Abrir agente de IA para este servidor", "Subir archivos", "Bajar archivos", "Editar", "Borrar"];
    let rows: Vec<Vec<String>> = actions.iter().map(|a| vec![a.to_string()]).collect();
    println!("{GRAY}{} → {}{RESET}", s.name, s.target());
    match picker::select_one(&["¿Qué quieres hacer?"], &rows) {
        Some(0) => connect(&s.name),
        Some(1) => crate::ai::agent_for_server(&s.name, &s.target()),
        Some(2) => upload(s),
        Some(3) => download(s),
        Some(4) => edit(Some(s)),
        Some(5) => {
            if !confirm(&format!("¿Borrar {} de ~/.ssh/config? (queda copia en config.fysh-respaldo)", s.name)) {
                return 0;
            }
            report(save(Some(&s.name), None), &format!("{} borrado", s.name))
        }
        _ => 0,
    }
}

// ---------- fysh dentro del servidor ----------

const INSTALL_URL: &str = "https://raw.githubusercontent.com/DryFernand/fysh/main/install.sh";

/// Servidores donde se eligió "no volver a preguntar" por instalar fysh.
fn no_fysh_file() -> Option<PathBuf> {
    home().map(|h| h.join(".fysh").join("ssh-sin-fysh"))
}

fn skips_fysh(name: &str) -> bool {
    no_fysh_file().and_then(|f| fs::read_to_string(f).ok()).is_some_and(|t| t.lines().any(|l| l.trim() == name))
}

fn remember_no_fysh(name: &str) {
    let Some(file) = no_fysh_file() else { return };
    let mut text = fs::read_to_string(&file).unwrap_or_default();
    text += &format!("{name}\n");
    let _ = file.parent().map(fs::create_dir_all);
    let _ = fs::write(file, text);
}

/// Ruta de fysh en el servidor (en el PATH o en ~/.local/bin).
/// Err si no se pudo conectar (ssh ya mostró el error).
fn remote_fysh(name: &str) -> Result<Option<String>, i32> {
    let script = r#"command -v fysh 2>/dev/null || { [ -x "$HOME/.local/bin/fysh" ] && echo "$HOME/.local/bin/fysh"; }"#;
    let out = std::process::Command::new("ssh")
        .args([name, script])
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|_| 127)?;
    if out.status.code() == Some(255) {
        return Err(255); // 255 = ssh no pudo conectar o autenticarse
    }
    let path = String::from_utf8_lossy(&out.stdout).lines().last().unwrap_or("").trim().to_string();
    Ok(path.starts_with('/').then_some(path))
}

/// Conectar: si el servidor tiene fysh, entra con fysh; si no, ofrece instalarlo
/// (en ~/.local/bin, sin sudo) o entrar normal.
fn connect(name: &str) -> i32 {
    let plain = || run_external(&["ssh".into(), name.into()]);
    if skips_fysh(name) || !std::io::stdin().is_terminal() {
        return plain();
    }
    println!("{GRAY}Buscando fysh en {name}…{RESET}");
    match remote_fysh(name) {
        Err(code) => code,
        Ok(Some(path)) => run_external(&["ssh".into(), "-t".into(), name.into(), path]),
        Ok(None) => {
            let rows = vec![
                vec!["Instalar fysh en el servidor y entrar con fysh".into(), format!("{GRAY}en ~/.local/bin, sin sudo{RESET}")],
                vec!["Entrar normal".into(), format!("{GRAY}la terminal del servidor (bash){RESET}")],
                vec!["Entrar normal y no volver a preguntar".into(), String::new()],
            ];
            println!("{GRAY}{name} no tiene fysh.{RESET}");
            match picker::select_one(&["¿Cómo entrar?", ""], &rows) {
                Some(0) => {
                    let install = format!("(curl -fsSL {INSTALL_URL} || wget -qO- {INSTALL_URL}) | sh");
                    if run_external(&["ssh".into(), name.into(), install]) != 0 {
                        eprintln!("{RED}No se pudo instalar fysh en {name}.{RESET} {GRAY}Entro normal.{RESET}");
                        return plain();
                    }
                    run_external(&["ssh".into(), "-t".into(), name.into(), "~/.local/bin/fysh".into()])
                }
                Some(1) => plain(),
                Some(2) => {
                    remember_no_fysh(name);
                    plain()
                }
                _ => 0,
            }
        }
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
    let identity = match choose_key(None, &base.identity) {
        Some(KeyChoice::Key(path)) => prepare_key(&path).display().to_string(),
        Some(KeyChoice::Keep) => base.identity.clone(),
        Some(KeyChoice::Password) => String::new(),
        None => return 0,
    };

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

// ---------- llaves privadas ----------

/// Carpetas donde suelen quedar las llaves (las descargadas de AWS, por ejemplo).
fn key_dirs() -> Vec<PathBuf> {
    let Some(h) = home() else { return Vec::new() };
    [".ssh", "Downloads", "Descargas", "Documents", "Documentos", "Desktop", "Escritorio", "OneDrive/Desktop", "OneDrive/Escritorio", "OneDrive/Documents", "OneDrive/Documentos"]
        .iter()
        .map(|d| h.join(d))
        .filter(|d| d.is_dir())
        .collect()
}

/// ¿Es una llave privada? Se mira el contenido, no la extensión.
fn is_private_key(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else { return false };
    if !meta.is_file() || meta.len() > 20_000 {
        return false;
    }
    let mut head = [0u8; 80];
    let n = fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    let text = String::from_utf8_lossy(&head[..n]);
    text.starts_with("-----BEGIN") && text.contains("PRIVATE KEY")
}

/// Llaves privadas en esas carpetas (y una subcarpeta adentro), sin repetir.
fn find_keys() -> Vec<(PathBuf, SystemTime)> {
    fn walk(dir: &Path, depth: u8, out: &mut Vec<(PathBuf, SystemTime)>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.filter_map(Result::ok) {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if depth > 0 && !name.starts_with('.') && name != "node_modules" {
                    walk(&path, depth - 1, out);
                }
            } else if is_private_key(&path) {
                let modified = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((path, modified));
            }
        }
    }
    let mut keys = Vec::new();
    for dir in key_dirs() {
        walk(&dir, 1, &mut keys);
    }
    let mut seen = Vec::new();
    keys.retain(|(p, _)| {
        let id = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let new = !seen.contains(&id);
        seen.push(id);
        new
    });
    keys
}

/// "mysql-bastion-key (1).pem" → "mysql-bastion-key": sin extensión ni el " (1)" que pone Windows
/// al descargar el mismo archivo otra vez.
fn base_name(path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    match stem.rsplit_once(" (") {
        Some((base, rest)) if rest.ends_with(')') && rest[..rest.len() - 1].chars().all(|c| c.is_ascii_digit()) => base.to_string(),
        _ => stem,
    }
}

enum KeyChoice {
    Key(PathBuf),
    /// Dejar la llave que ya tenía el servidor.
    Keep,
    Password,
}

/// Selector de llaves. `wanted`: la ruta que se escribió (sus parecidas van primero).
/// `current`: llave actual del servidor en el asistente (se ofrece mantenerla).
/// Con `wanted`, no se ofrece contraseña: quien escribió -i quiere una llave.
fn choose_key(wanted: Option<&Path>, current: &str) -> Option<KeyChoice> {
    let mut keys = find_keys();
    let target = wanted.map(base_name);
    keys.sort_by(|a, b| {
        let a_match = target.as_deref() == Some(base_name(&a.0).as_str());
        let b_match = target.as_deref() == Some(base_name(&b.0).as_str());
        b_match.cmp(&a_match).then(b.1.cmp(&a.1))
    });

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut choices: Vec<KeyChoice> = Vec::new();
    if !current.is_empty() {
        rows.push(vec!["Mantener la actual".into(), format!("{GRAY}{current}{RESET}"), String::new()]);
        choices.push(KeyChoice::Keep);
    }
    if wanted.is_none() {
        rows.push(vec!["Sin llave (usar contraseña)".into(), String::new(), String::new()]);
        choices.push(KeyChoice::Password);
    }
    for (path, modified) in keys {
        let folder = path.parent().map(tilde).unwrap_or_default();
        let when = chrono::DateTime::<chrono::Local>::from(modified).format("%Y-%m-%d").to_string();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        rows.push(vec![name, format!("{GRAY}{folder}{RESET}"), format!("{GRAY}{when}{RESET}")]);
        choices.push(KeyChoice::Key(path));
    }

    if choices.iter().all(|c| !matches!(c, KeyChoice::Key(_))) && wanted.is_some() {
        eprintln!("No encontré ninguna llave privada en ~/.ssh, Descargas, Documentos ni Escritorio.");
        eprintln!("{GRAY}Descarga la llave (.pem) y vuelve a intentar.{RESET}");
        return None;
    }
    println!("{GRAY}Elige la llave para conectar:{RESET}");
    let i = picker::select_one(&["Llave", "Carpeta", "Fecha"], &rows)?;
    choices.into_iter().nth(i)
}

fn inside_ssh_dir(path: &Path) -> bool {
    let Some(ssh) = home().map(|h| h.join(".ssh")) else { return false };
    match (fs::canonicalize(path), fs::canonicalize(ssh)) {
        (Ok(p), Ok(s)) => p.starts_with(s),
        _ => false,
    }
}

/// Deja la llave lista para usar: si está fuera de ~/.ssh ofrece copiarla allí, y la protege
/// para que solo tú puedas leerla (OpenSSH rechaza llaves con "UNPROTECTED PRIVATE KEY FILE").
fn prepare_key(path: &Path) -> PathBuf {
    let mut key = path.to_path_buf();
    if !inside_ssh_dir(path) {
        let folder = path.parent().map(tilde).unwrap_or_default();
        if confirm(&format!("La llave está en {folder}. ¿Copiarla a ~/.ssh y protegerla? (recomendado)")) {
            if let Some(ssh) = home().map(|h| h.join(".ssh")) {
                let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                let target = ssh.join(format!("{}{ext}", base_name(path)));
                let same = fs::read(&target).ok() == fs::read(path).ok();
                let copied = same || (!target.exists() && fs::create_dir_all(&ssh).and_then(|_| fs::copy(path, &target)).is_ok());
                if copied {
                    println!("{GREEN}✓{RESET} Llave copiada a {}", tilde(&target));
                    key = target;
                } else {
                    eprintln!("{GRAY}Ya existe otra llave llamada {} en ~/.ssh: uso la original.{RESET}", tilde(&target));
                }
            }
        }
    }
    if protect(&key) {
        println!("{GREEN}✓{RESET} Permisos de la llave ajustados: solo tú puedes leerla");
    }
    key
}

/// Solo el usuario actual puede leer la llave.
fn protect(path: &Path) -> bool {
    #[cfg(windows)]
    {
        let Ok(user) = std::env::var("USERNAME") else { return false };
        std::process::Command::new("icacls")
            .arg(path)
            .args(["/inheritance:r", "/grant:r", &format!("{user}:F")])
            .output()
            .is_ok_and(|o| o.status.success())
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).is_ok()
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    match (path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")), home()) {
        (Some(rest), Some(h)) => h.join(rest),
        _ => PathBuf::from(path),
    }
}

/// Opciones de ssh que llevan un valor después (para encontrar el destino).
const OPTIONS_WITH_VALUE: &[&str] =
    &["-i", "-p", "-l", "-o", "-F", "-J", "-L", "-R", "-D", "-b", "-c", "-E", "-e", "-m", "-O", "-Q", "-S", "-W", "-w", "-B"];

/// Destino de `ssh ... usuario@host ...` como servidor (sin nombre todavía). None si no hay destino.
fn destination(args: &[String]) -> Option<Server> {
    let mut server = Server { port: 22, ..Default::default() };
    let mut i = 0;
    let mut dest = None;
    while i < args.len() {
        let a = args[i].as_str();
        if OPTIONS_WITH_VALUE.contains(&a) {
            let value = args.get(i + 1).cloned().unwrap_or_default();
            match a {
                "-p" => server.port = value.parse().unwrap_or(22),
                "-l" => server.user = value,
                "-i" => server.identity = value,
                _ => {}
            }
            i += 2;
        } else if a.starts_with('-') {
            i += 1;
        } else {
            dest = Some(a.to_string());
            break;
        }
    }
    let dest = dest?;
    match dest.split_once('@') {
        Some((user, host)) => {
            server.user = user.into();
            server.host = host.into();
        }
        None => server.host = dest,
    }
    Some(server)
}

/// `ssh <argumentos>`: el ssh real, con ayuda para la llave y para guardar el servidor.
pub fn ssh_args(args: &[String]) -> i32 {
    // `ssh <servidor guardado>` entra igual que "Conectar" del selector (con fysh si lo tiene).
    if let [name] = args {
        if load().iter().any(|s| &s.name == name) {
            return connect(name);
        }
    }
    let mut args = args.to_vec();
    // "-ipath" → "-i", "path"
    if let Some(i) = args.iter().position(|a| a.starts_with("-i") && a.len() > 2) {
        let path = args[i][2..].to_string();
        args.splice(i..=i, ["-i".to_string(), path]);
    }
    if let Some(i) = args.iter().position(|a| a == "-i").filter(|i| i + 1 < args.len()) {
        let written = expand_tilde(&args[i + 1]);
        let key = if written.is_file() {
            // Existe pero fuera de ~/.ssh (p. ej. en Descargas): ofrecer copiarla y protegerla.
            if inside_ssh_dir(&written) { written } else { prepare_key(&written) }
        } else {
            eprintln!("{RED}No existe la llave:{RESET} {}", args[i + 1]);
            match choose_key(Some(&written), "") {
                Some(KeyChoice::Key(path)) => prepare_key(&path),
                _ => return 1,
            }
        };
        args[i + 1] = key.display().to_string();
    }

    let mut cmd = vec!["ssh".to_string()];
    cmd.extend(args.iter().cloned());
    let code = run_external(&cmd);
    if code == 0 {
        offer_save(&args);
    }
    code
}

/// Después de una conexión que funcionó: si el servidor no estaba guardado, ofrecer guardarlo.
fn offer_save(args: &[String]) {
    let Some(mut server) = destination(args) else { return };
    let saved = load();
    if saved.iter().any(|s| s.name == server.host || (s.host == server.host && s.user == server.user)) {
        return;
    }
    if !std::io::stdin().is_terminal() || !confirm("¿Guardar este servidor para conectarte la próxima vez desde el selector de ssh?") {
        return;
    }
    let suggestion = server.host.split('.').next().unwrap_or("servidor").to_string();
    let Some(name) = ask("Nombre corto: ", &suggestion) else { return };
    if name.contains(char::is_whitespace) || saved.iter().any(|s| s.name == name) {
        eprintln!("Nombre no válido o ya usado: {name}");
        return;
    }
    server.name = name.clone();
    server.identity = server.identity.replace('\\', "/");
    report(save(None, Some(&server)), &format!("{name} guardado → {}", server.target()));
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
    fn reconoce_descargas_repetidas() {
        assert_eq!(base_name(Path::new("mysql-bastion-key (1).pem")), "mysql-bastion-key");
        assert_eq!(base_name(Path::new("/root/.ssh/mysql-bastion-key.pem")), "mysql-bastion-key");
        assert_eq!(base_name(Path::new("id_ed25519")), "id_ed25519");
        assert_eq!(base_name(Path::new("llave (vieja).pem")), "llave (vieja)");
    }

    #[test]
    fn entiende_el_destino_de_ssh() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let s = destination(&args(&["-i", "k.pem", "ubuntu@ec2-1-2-3-4.compute-1.amazonaws.com"])).unwrap();
        assert_eq!((s.user.as_str(), s.host.as_str(), s.port, s.identity.as_str()), ("ubuntu", "ec2-1-2-3-4.compute-1.amazonaws.com", 22, "k.pem"));
        let s = destination(&args(&["-p", "2222", "-l", "root", "-v", "10.0.0.5", "uptime"])).unwrap();
        assert_eq!((s.user.as_str(), s.host.as_str(), s.port), ("root", "10.0.0.5", 2222));
        assert!(destination(&args(&["-V"])).is_none());
    }

    #[test]
    fn detecta_llaves_por_contenido() {
        let dir = std::env::temp_dir().join(format!("fysh-keys-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("aws.pem"), "-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n").unwrap();
        fs::write(dir.join("id_ed25519.pub"), "ssh-ed25519 AAAA usuario").unwrap();
        fs::write(dir.join("notas.pem"), "no es una llave").unwrap();
        assert!(is_private_key(&dir.join("aws.pem")));
        assert!(!is_private_key(&dir.join("id_ed25519.pub")));
        assert!(!is_private_key(&dir.join("notas.pem")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn config_vacio() {
        let s = Server { name: "a".into(), host: "1.2.3.4".into(), port: 22, ..Default::default() };
        assert_eq!(update_config("", None, Some(&s)), "Host a\n    HostName 1.2.3.4\n    Port 22\n");
        assert_eq!(update_config("Host a\n    HostName 1.2.3.4\n", Some("a"), None), "");
    }
}
