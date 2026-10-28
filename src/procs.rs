//! Procesos y puertos.
//! `procesos [texto]`: los procesos que más memoria usan, para cerrar uno.
//! `puertos [número]`: qué programa está escuchando en cada puerto (el 3000 de `npm run dev`,
//! el 8000 de Django…), para abrirlo en el navegador o cerrarlo.

use crate::ask::confirm;
use crate::picker;
use crate::table::{self, truncate, Align};
use crate::{GRAY, GREEN, RED, RESET, YELLOW};
use std::collections::BTreeMap;
use std::io::IsTerminal;
use sysinfo::{Pid, ProcessesToUpdate, System};

pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub memory: u64,
    pub cpu: f32,
}

/// Todos los procesos. `with_cpu` mide el uso de CPU (espera 200 ms entre dos lecturas).
fn list(with_cpu: bool) -> (System, Vec<Proc>) {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    if with_cpu {
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(std::time::Duration::from_millis(200)));
        sys.refresh_processes(ProcessesToUpdate::All, true);
    }
    let cores = sys.cpus().len().max(1) as f32;
    let procs = sys
        .processes()
        .values()
        .map(|p| Proc {
            pid: p.pid().as_u32(),
            name: p.name().to_string_lossy().into_owned(),
            memory: p.memory(),
            cpu: p.cpu_usage() / if with_cpu { cores } else { 1.0 },
        })
        .collect();
    (sys, procs)
}

/// Cierra el proceso, después de preguntar.
fn kill(sys: &System, pid: u32, name: &str) -> i32 {
    if !confirm(&format!("¿Cerrar {name} (pid {pid})? Lo que no esté guardado en ese programa se pierde.")) {
        return 0;
    }
    match sys.process(Pid::from_u32(pid)) {
        Some(p) if p.kill() => {
            println!("{GREEN}✓{RESET} {name} cerrado");
            0
        }
        _ => {
            eprintln!("{RED}No se pudo cerrar {name}.{RESET} {GRAY}Puede que sea del sistema o de otro usuario (prueba como administrador).{RESET}");
            1
        }
    }
}

fn mem(bytes: u64) -> String {
    let mb = bytes as f64 / 1024.0 / 1024.0;
    if mb >= 1024.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{mb:.0} MB") }
}

/// `procesos [texto]`: los 40 que más memoria usan (o los que contienen el texto).
pub fn procesos_cmd(args: &[String]) -> i32 {
    let filter = args.join(" ").to_lowercase();
    let (sys, mut procs) = list(true);
    procs.retain(|p| filter.is_empty() || p.name.to_lowercase().contains(&filter) || p.pid.to_string() == filter);
    procs.sort_by(|a, b| b.memory.cmp(&a.memory));
    procs.truncate(40);
    if procs.is_empty() {
        println!("{GRAY}No hay procesos que coincidan con \"{filter}\".{RESET}");
        return 1;
    }
    let rows: Vec<Vec<String>> = procs
        .iter()
        .map(|p| vec![truncate(&p.name, 35), format!("{GRAY}{}{RESET}", p.pid), mem(p.memory), format!("{:.1}%", p.cpu)])
        .collect();
    let headers = ["Proceso", "PID", "Memoria", "CPU"];
    if !std::io::stdin().is_terminal() {
        table::print(&headers, &[Align::Left, Align::Right, Align::Right, Align::Right], &rows);
        return 0;
    }
    println!("{GRAY}Enter sobre un proceso para cerrarlo · Esc para salir{RESET}");
    match picker::select_one(&headers, &rows) {
        Some(i) => kill(&sys, procs[i].pid, &procs[i].name),
        None => 0,
    }
}

/// Un puerto escuchando, con el proceso dueño.
struct Listener {
    port: u16,
    addrs: Vec<String>,
    pid: u32,
}

fn listeners() -> Result<Vec<Listener>, String> {
    use netstat2::{get_sockets_info, AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState};
    let sockets = get_sockets_info(AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6, ProtocolFlags::TCP)
        .map_err(|e| format!("no se pudieron leer los puertos: {e}"))?;
    // Juntar IPv4 e IPv6 del mismo puerto y proceso en una sola fila.
    let mut by_key: BTreeMap<(u16, u32), Vec<String>> = BTreeMap::new();
    for s in sockets {
        let ProtocolSocketInfo::Tcp(tcp) = s.protocol_socket_info else { continue };
        if tcp.state != TcpState::Listen {
            continue;
        }
        for pid in s.associated_pids {
            let addrs = by_key.entry((tcp.local_port, pid)).or_default();
            let addr = tcp.local_addr.to_string();
            if !addrs.contains(&addr) {
                addrs.push(addr);
            }
        }
    }
    Ok(by_key.into_iter().map(|((port, pid), addrs)| Listener { port, addrs, pid }).collect())
}

/// "0.0.0.0, ::" → "todas las redes"; "127.0.0.1" → "solo este equipo".
fn reach(addrs: &[String]) -> String {
    if addrs.iter().any(|a| a == "0.0.0.0" || a == "::") {
        format!("{YELLOW}toda la red{RESET}")
    } else if addrs.iter().all(|a| a == "127.0.0.1" || a == "::1") {
        format!("{GRAY}solo este equipo{RESET}")
    } else {
        addrs.join(", ")
    }
}

/// Puertos del sistema operativo que no interesan para desarrollar: servicios de Windows y la
/// zona de puertos dinámicos (49152+), que el sistema reparte a cualquier programa.
fn is_system(port: u16, name: &str) -> bool {
    const SYSTEM: &[&str] = &["system", "svchost.exe", "lsass.exe", "wininit.exe", "services.exe", "spoolsv.exe", "launchd", "systemd", "systemd-resolved"];
    port >= 49152 || SYSTEM.contains(&name.to_lowercase().as_str())
}

/// `puertos [número | --todos]`: qué escucha en cada puerto; Enter para abrir en el navegador o cerrar.
pub fn puertos_cmd(args: &[String]) -> i32 {
    let all = args.iter().any(|a| a == "--todos" || a == "-a");
    let args: Vec<String> = args.iter().filter(|a| !a.starts_with('-')).cloned().collect();
    let wanted: Option<u16> = match args.first() {
        Some(a) => match a.parse() {
            Ok(n) => Some(n),
            Err(_) => {
                eprintln!("{GRAY}uso: puertos [número]   ej: puertos 3000{RESET}");
                return 1;
            }
        },
        None => None,
    };
    let mut found = match listeners() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("puertos: {e}");
            return 1;
        }
    };
    if let Some(port) = wanted {
        found.retain(|l| l.port == port);
        if found.is_empty() {
            println!("{GREEN}✓{RESET} El puerto {port} está libre.");
            return 0;
        }
    }
    let (sys, procs) = list(false);
    let name_of = |pid: u32| procs.iter().find(|p| p.pid == pid).map_or("(desconocido)".to_string(), |p| p.name.clone());
    let before = found.len();
    if !all && wanted.is_none() {
        found.retain(|l| !is_system(l.port, &name_of(l.pid)));
    }
    let hidden = before - found.len();
    if found.is_empty() {
        println!("{GRAY}No hay programas tuyos escuchando en ningún puerto ({hidden} del sistema ocultos: puertos --todos).{RESET}");
        return 0;
    }
    let rows: Vec<Vec<String>> = found
        .iter()
        .map(|l| vec![format!("{YELLOW}{}{RESET}", l.port), truncate(&name_of(l.pid), 30), format!("{GRAY}{}{RESET}", l.pid), reach(&l.addrs)])
        .collect();
    let headers = ["Puerto", "Programa", "PID", "Acceso"];
    let footer = || {
        if hidden > 0 {
            println!("{GRAY}{hidden} puertos del sistema ocultos · puertos --todos para verlos{RESET}");
        }
    };
    if !std::io::stdin().is_terminal() {
        table::print(&headers, &[Align::Right, Align::Left, Align::Right, Align::Left], &rows);
        footer();
        return 0;
    }
    footer();
    let i = if found.len() == 1 && wanted.is_some() {
        table::print(&headers, &[Align::Right, Align::Left, Align::Right, Align::Left], &rows);
        0
    } else {
        println!("{GRAY}Enter sobre un puerto para abrirlo o cerrarlo · Esc para salir{RESET}");
        match picker::select_one(&headers, &rows) {
            Some(i) => i,
            None => return 0,
        }
    };
    let l = &found[i];
    let name = name_of(l.pid);
    let actions = vec![
        vec![format!("Abrir http://localhost:{} en el navegador", l.port)],
        vec![format!("Cerrar {name} (pid {}) para liberar el puerto", l.pid)],
    ];
    match picker::select_one(&["¿Qué quieres hacer?"], &actions) {
        Some(0) => crate::files::open(&[format!("http://localhost:{}", l.port)]),
        Some(1) => kill(&sys, l.pid, &name),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_el_acceso() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(reach(&v(&["0.0.0.0", "::"])).contains("toda la red"));
        assert!(reach(&v(&["127.0.0.1", "::1"])).contains("solo este equipo"));
        assert_eq!(reach(&v(&["192.168.1.5"])), "192.168.1.5");
        assert_eq!(mem(512 * 1024 * 1024), "512 MB");
        assert_eq!(mem(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert!(is_system(135, "svchost.exe") && is_system(49690, "agy.EXE") && is_system(445, "System"));
        assert!(!is_system(3000, "node.exe") && !is_system(3306, "mysqld.exe"));
    }

    #[test]
    fn encuentra_su_propio_proceso() {
        let (_, procs) = list(false);
        let me = std::process::id();
        assert!(procs.iter().any(|p| p.pid == me), "debe listar el proceso de los tests");
        assert!(listeners().is_ok());
    }
}
