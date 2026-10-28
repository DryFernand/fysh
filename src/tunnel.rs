//! `tunel`: túneles SSH guardados. Un túnel hace que un puerto de tu PC (ej. 3307) llegue a un
//! servicio que solo se ve desde un servidor (ej. la base de datos de AWS detrás del bastión):
//! `ssh -N -L 3307:mi-db.rds.amazonaws.com:3306 bastion`.
//! Los túneles corren en segundo plano y siguen activos aunque cierres fysh; se apagan con `tunel`.
//! Se guardan en ~/.fysh/tuneles.json.

use crate::ask::{ask, confirm};
use crate::picker;
use crate::table::{self, Align};
use crate::{home, GRAY, GREEN, RED, RESET, YELLOW};
use serde_json::{json, Value};
use std::io::IsTerminal;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Tunnel {
    pub name: String,
    /// Servidor ssh guardado por el que pasa el túnel.
    pub via: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    /// Proceso ssh mientras está activo.
    pub pid: Option<u32>,
}

impl Tunnel {
    fn route(&self) -> String {
        format!("localhost:{} → {}:{}", self.local_port, self.remote_host, self.remote_port)
    }

    fn to_json(&self) -> Value {
        json!({"name": self.name, "via": self.via, "local_port": self.local_port,
               "remote_host": self.remote_host, "remote_port": self.remote_port, "pid": self.pid})
    }

    fn from_json(v: &Value) -> Option<Self> {
        Some(Tunnel {
            name: v["name"].as_str()?.into(),
            via: v["via"].as_str()?.into(),
            local_port: v["local_port"].as_u64()? as u16,
            remote_host: v["remote_host"].as_str()?.into(),
            remote_port: v["remote_port"].as_u64()? as u16,
            pid: v["pid"].as_u64().map(|p| p as u32),
        })
    }
}

fn data_file() -> Option<PathBuf> {
    home().map(|h| h.join(".fysh").join("tuneles.json"))
}

pub fn load() -> Vec<Tunnel> {
    data_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok())
        .map(|list| list.iter().filter_map(Tunnel::from_json).collect())
        .unwrap_or_default()
}

fn save(list: &[Tunnel]) -> std::io::Result<()> {
    let file = data_file().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "sin carpeta de usuario"))?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&list.iter().map(Tunnel::to_json).collect::<Vec<_>>()).unwrap_or_default();
    std::fs::write(file, text)
}

/// ¿Sigue vivo el ssh de este túnel?
fn is_active(t: &Tunnel) -> bool {
    let Some(pid) = t.pid else { return false };
    let mut sys = sysinfo::System::new();
    let pid = sysinfo::Pid::from_u32(pid);
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).is_some_and(|p| p.name().to_string_lossy().to_lowercase().starts_with("ssh"))
}

/// Cuántos túneles están activos (para el prompt).
pub fn active_count() -> usize {
    let list = load();
    if list.iter().all(|t| t.pid.is_none()) {
        return 0;
    }
    list.iter().filter(|t| is_active(t)).count()
}

fn port_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn log_file(name: &str) -> Option<PathBuf> {
    home().map(|h| h.join(".fysh").join(format!("tunel-{name}.log")))
}

/// Lanza el ssh en segundo plano y espera a que el puerto local responda. Devuelve el pid.
pub fn start(t: &Tunnel) -> Result<u32, String> {
    if !port_free(t.local_port) {
        return Err(format!("el puerto {} ya está en uso (míralo con: puertos {})", t.local_port, t.local_port));
    }
    let log = log_file(&t.name).ok_or("sin carpeta de usuario")?;
    let _ = log.parent().map(std::fs::create_dir_all);
    let out = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-N",
        "-L",
        &format!("127.0.0.1:{}:{}:{}", t.local_port, t.remote_host, t.remote_port),
        "-o",
        "BatchMode=yes", // sin contraseña: en segundo plano no hay dónde escribirla
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ServerAliveInterval=30",
        &t.via,
    ])
    .stdin(Stdio::null())
    .stdout(out)
    .stderr(err);
    let mut child = spawn_detached(&mut cmd).map_err(|e| format!("no se pudo iniciar ssh: {e}"))?;

    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            let reason = text.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("ssh terminó sin decir por qué").to_string();
            let hint = if text.contains("Permission denied") {
                " — los túneles necesitan entrar con llave (el servidor pidió contraseña)"
            } else {
                ""
            };
            return Err(format!("{reason}{hint}"));
        }
        if TcpStream::connect_timeout(&([127, 0, 0, 1], t.local_port).into(), Duration::from_millis(300)).is_ok() {
            return Ok(child.id());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let _ = child.kill();
    Err("el túnel no respondió a tiempo".into())
}

/// El ssh sigue vivo aunque se cierre la ventana de fysh.
#[cfg(windows)]
fn spawn_detached(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::{GetHandleInformation, SetHandleInformation, HANDLE_FLAG_INHERIT};
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    // Windows hereda al hijo todo lo heredable, incluida la salida de quien lanzó fysh (un pipe
    // de PowerShell, por ejemplo): el túnel lo dejaría abierto y ese programa esperaría para
    // siempre. Mientras se lanza ssh, la entrada/salida de fysh no se hereda.
    let handles: Vec<_> = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
        .iter()
        .map(|&id| unsafe {
            let h = GetStdHandle(id);
            let mut flags = 0u32;
            GetHandleInformation(h, &mut flags);
            SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            (h, flags & HANDLE_FLAG_INHERIT)
        })
        .collect();
    // Salir del "job" de la terminal si se puede, para que cerrarla no cierre el túnel.
    let result = cmd
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
        .or_else(|_| cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW).spawn());
    for (h, inherit) in handles {
        unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, inherit) };
    }
    result
}

#[cfg(not(windows))]
fn spawn_detached(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0).spawn()
}

fn stop(t: &Tunnel) -> bool {
    let Some(pid) = t.pid else { return true };
    let mut sys = sysinfo::System::new();
    let pid = sysinfo::Pid::from_u32(pid);
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).map_or(true, |p| p.kill())
}

/// Activa el túnel `name` si no lo está. Lo usa `db` antes de conectar. Devuelve el puerto local.
pub fn ensure_active(name: &str) -> Result<u16, String> {
    let mut list = load();
    let i = list.iter().position(|t| t.name == name).ok_or_else(|| format!("no existe el túnel {name}"))?;
    if is_active(&list[i]) {
        return Ok(list[i].local_port);
    }
    println!("{GRAY}Activando el túnel {name}…{RESET}");
    let pid = start(&list[i])?;
    list[i].pid = Some(pid);
    save(&list).map_err(|e| e.to_string())?;
    Ok(list[i].local_port)
}

pub fn names() -> Vec<String> {
    load().into_iter().map(|t| t.name).collect()
}

/// `tunel` (selector) o `tunel <nombre>` (activar/desactivar).
pub fn tunel_cmd(args: &[String]) -> i32 {
    let mut list = load();
    if let Some(name) = args.first() {
        let Some(i) = list.iter().position(|t| &t.name == name) else {
            eprintln!("tunel: no existe \"{name}\". Escribe tunel para ver la lista.");
            return 1;
        };
        return toggle(&mut list, i);
    }
    let states: Vec<bool> = list.iter().map(is_active).collect();
    let rows: Vec<Vec<String>> = list
        .iter()
        .zip(&states)
        .map(|(t, &on)| {
            let state = if on { format!("{GREEN}● activo{RESET}") } else { format!("{GRAY}○ apagado{RESET}") };
            vec![t.name.clone(), format!("{YELLOW}{}{RESET}", t.route()), format!("{GRAY}vía {}{RESET}", t.via), state]
        })
        .collect();
    let headers = ["Túnel", "Ruta", "Servidor", "Estado"];
    if !std::io::stdin().is_terminal() {
        table::print(&headers, &[Align::Left; 4], &rows);
        return 0;
    }
    let mut options = vec![vec!["+ Nuevo túnel".to_string(), String::new(), String::new(), String::new()]];
    options.extend(rows);
    match picker::select_one(&headers, &options) {
        Some(0) => create(&mut list),
        Some(i) => tunnel_menu(&mut list, i - 1, states[i - 1]),
        None => 0,
    }
}

fn toggle(list: &mut [Tunnel], i: usize) -> i32 {
    if is_active(&list[i]) {
        stop(&list[i]);
        list[i].pid = None;
        let _ = save(list);
        println!("{GREEN}✓{RESET} Túnel {} apagado", list[i].name);
        return 0;
    }
    match start(&list[i]) {
        Ok(pid) => {
            list[i].pid = Some(pid);
            let _ = save(list);
            println!("{GREEN}✓{RESET} Túnel {} activo: {} {GRAY}(sigue activo aunque cierres fysh){RESET}", list[i].name, list[i].route());
            0
        }
        Err(e) => {
            eprintln!("{RED}No se pudo activar {}:{RESET} {e}", list[i].name);
            1
        }
    }
}

fn tunnel_menu(list: &mut Vec<Tunnel>, i: usize, active: bool) -> i32 {
    let actions = [if active { "Apagar" } else { "Activar" }, "Borrar"];
    let rows: Vec<Vec<String>> = actions.iter().map(|a| vec![a.to_string()]).collect();
    println!("{GRAY}{} → {} vía {}{RESET}", list[i].name, list[i].route(), list[i].via);
    match picker::select_one(&["¿Qué quieres hacer?"], &rows) {
        Some(0) => toggle(list, i),
        Some(1) => {
            if !confirm(&format!("¿Borrar el túnel {}?", list[i].name)) {
                return 0;
            }
            stop(&list[i]);
            let name = list.remove(i).name;
            match save(list) {
                Ok(()) => {
                    println!("{GREEN}✓{RESET} Túnel {name} borrado");
                    0
                }
                Err(e) => {
                    eprintln!("tunel: no se pudo guardar: {e}");
                    1
                }
            }
        }
        _ => 0,
    }
}

/// Asistente: servidor de salto, destino y puerto local.
fn create(list: &mut Vec<Tunnel>) -> i32 {
    let servers = crate::ssh::host_names();
    if servers.is_empty() {
        eprintln!("Primero guarda el servidor por el que pasa el túnel: escribe ssh → + Nuevo servidor.");
        return 1;
    }
    println!("{GRAY}¿Por qué servidor pasa el túnel? (el que sí puede ver el destino, ej. el bastión){RESET}");
    let rows: Vec<Vec<String>> = servers.iter().map(|s| vec![s.clone()]).collect();
    let Some(si) = picker::select_one(&["Servidor"], &rows) else { return 0 };
    let via = servers[si].clone();

    let Some(remote_host) = ask("Destino visto desde el servidor (ej: mi-db.xxxx.rds.amazonaws.com, o localhost): ", "localhost") else { return 0 };
    let Some(remote_port) = ask("Puerto del destino (MySQL 3306, PostgreSQL 5432): ", "3306").and_then(|p| p.parse::<u16>().ok()) else {
        eprintln!("Puerto no válido.");
        return 1;
    };
    let used: Vec<u16> = list.iter().map(|t| t.local_port).collect();
    let suggested = (remote_port.saturating_add(1)..u16::MAX).find(|p| !used.contains(p) && port_free(*p)).unwrap_or(remote_port);
    let Some(local_port) = ask("Puerto en tu PC: ", &suggested.to_string()).and_then(|p| p.parse::<u16>().ok()) else {
        eprintln!("Puerto no válido.");
        return 1;
    };
    let default_name = format!("{via}-{remote_port}");
    let Some(name) = ask("Nombre del túnel: ", &default_name) else { return 0 };
    if name.contains(char::is_whitespace) || list.iter().any(|t| t.name == name) {
        eprintln!("Nombre no válido o ya usado: {name}");
        return 1;
    }
    list.push(Tunnel { name: name.clone(), via, local_port, remote_host, remote_port, pid: None });
    if let Err(e) = save(list) {
        eprintln!("tunel: no se pudo guardar: {e}");
        return 1;
    }
    let route = list.last().map(Tunnel::route).unwrap_or_default();
    println!("{GREEN}✓{RESET} Túnel {name} guardado: {route}");
    if confirm("¿Activarlo ahora?") {
        let i = list.len() - 1;
        return toggle(list, i);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarda_y_lee_tuneles() {
        let t = Tunnel { name: "bastion-3306".into(), via: "bastion".into(), local_port: 3307, remote_host: "db.rds.amazonaws.com".into(), remote_port: 3306, pid: Some(42) };
        assert_eq!(Tunnel::from_json(&t.to_json()), Some(t.clone()));
        assert_eq!(t.route(), "localhost:3307 → db.rds.amazonaws.com:3306");
        let apagado = Tunnel { pid: None, ..t };
        assert_eq!(Tunnel::from_json(&apagado.to_json()).unwrap().pid, None);
    }

    #[test]
    fn detecta_puertos_libres() {
        let ocupado = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = ocupado.local_addr().unwrap().port();
        assert!(!port_free(port));
        drop(ocupado);
        assert!(port_free(port));
    }
}
