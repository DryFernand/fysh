use crate::table::{self, Align};
use crate::{Shell, BLUE, GRAY, RESET};
use std::env;
use std::fs;
use std::io::{self, Write};

/// Comandos internos. Devuelve Some(código) si lo manejó, None si es externo.
/// Para agregar un comando nuevo: añade una línea al match y escribe su función.
pub fn run(args: &[String], shell: &mut Shell) -> Option<i32> {
    let rest = &args[1..];
    let code = match args[0].as_str() {
        "exit" => exit(rest, shell),
        "cd" => cd(rest),
        "pwd" => pwd(),
        "echo" => echo(rest),
        "set" => set(rest),
        "cls" | "clear" => cls(),
        "dir" | "ls" => dir(rest),
        "help" => help(),
        "mkdir" => crate::files::mkdir(rest),
        "rm" => crate::files::rm(rest),
        "cp" => crate::files::cp(rest),
        "mv" => crate::files::mv(rest),
        "cat" => crate::files::cat(rest),
        "touch" => crate::files::touch(rest),
        "open" => crate::files::open(rest),
        "alias" => {
            let raw = shell.line.trim_start().strip_prefix("alias").unwrap_or("").to_string();
            crate::aliases::alias_cmd(shell, &raw)
        }
        "unalias" => crate::aliases::unalias_cmd(shell, rest),
        "config" => crate::aliases::config_cmd(shell),
        "z" => crate::jump::z_cmd(rest),
        "run" => crate::run::run_cmd(shell, rest),
        "docker" if rest.is_empty() => crate::docker::docker_cmd(),
        "tunel" => crate::tunnel::tunel_cmd(rest),
        "db" => crate::db::db_cmd(rest),
        "http" => crate::http::http_cmd(shell, rest),
        "procesos" => crate::procs::procesos_cmd(rest),
        "puertos" => crate::procs::puertos_cmd(rest),
        "ssh" if rest.is_empty() => crate::ssh::ssh_cmd(),
        "ssh" => crate::ssh::ssh_args(rest),
        "agent" => crate::ai::agent_cmd(),
        "actualizar" | "update" => crate::update::actualizar_cmd(),
        "explica" => crate::ai::explica_cmd(shell),
        cmd if cmd.starts_with("ins:") => {
            let task = shell.line.trim_start().strip_prefix("ins:").unwrap_or("").to_string();
            crate::ai::ins_cmd(shell, &task)
        }
        "env" if rest.is_empty() => crate::envvars::env_cmd(),
        "path" if rest.is_empty() => crate::envvars::path_cmd(),
        "claude" if rest.is_empty() => crate::agents::claude_picker(),
        "agy" | "antigravity" if rest.is_empty() => crate::agents::agy_picker(),
        "agy" | "antigravity" if matches!(rest, [a] if a == "-rename" || a == "--rename") => crate::agents::agy_rename(),
        "agy" | "antigravity" if matches!(rest, [a] if a == "-delete" || a == "--delete") => crate::agents::agy_delete(),
        "opencode" => crate::agents::opencode(rest),
        "git" => return crate::git::run(rest),
        _ => return None,
    };
    Some(code)
}

fn exit(args: &[String], shell: &mut Shell) -> i32 {
    shell.running = false;
    args.first().and_then(|a| a.parse().ok()).unwrap_or(0)
}

fn cd(args: &[String]) -> i32 {
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let target = match args.first() {
        Some(a) if a.starts_with('~') => format!("{home}{}", &a[1..]),
        Some(a) => a.clone(),
        None => home,
    };
    match env::set_current_dir(&target) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("cd: {target}: {}", error_msg(&e));
            1
        }
    }
}

fn pwd() -> i32 {
    match env::current_dir() {
        Ok(p) => {
            println!("{}", p.display());
            0
        }
        Err(e) => {
            eprintln!("pwd: {e}");
            1
        }
    }
}

fn echo(args: &[String]) -> i32 {
    println!("{}", args.join(" "));
    0
}

/// set NOMBRE=valor  → define variable de entorno. Sin argumentos, las lista en tabla.
fn set(args: &[String]) -> i32 {
    if args.is_empty() {
        const MAX: usize = 70;
        let mut vars: Vec<(String, String)> = env::vars().collect();
        vars.sort_by_key(|(k, _)| k.to_lowercase());
        let mut cut = false;
        let rows: Vec<Vec<String>> = vars
            .into_iter()
            .map(|(k, v)| {
                let v = if v.chars().count() > MAX {
                    cut = true;
                    format!("{}…", v.chars().take(MAX - 1).collect::<String>())
                } else {
                    v
                };
                vec![k, v]
            })
            .collect();
        table::print(&["Variable", "Valor"], &[Align::Left, Align::Left], &rows);
        if cut {
            println!("{GRAY}Valores largos recortados (…). Usa echo $NOMBRE para verlos completos.{RESET}");
        }
        return 0;
    }
    for a in args {
        match a.split_once('=') {
            Some((k, v)) => env::set_var(k, v),
            None => {
                eprintln!("set: formato esperado NOMBRE=valor");
                return 1;
            }
        }
    }
    0
}

/// Limpia la pantalla y el historial de scroll.
fn cls() -> i32 {
    print!("\x1b[2J\x1b[3J\x1b[H");
    io::stdout().flush().ok();
    0
}

/// Lista una carpeta en tabla: primero carpetas, luego archivos, por nombre.
/// Con archivos (`dir *.txt`, que el comodín convierte en la lista de archivos) muestra solo esos.
fn dir(args: &[String]) -> i32 {
    struct Item {
        name: String,
        is_dir: bool,
        size: u64,
        modified: Option<std::time::SystemTime>,
    }
    let item = |name: String, meta: Option<fs::Metadata>| Item {
        name,
        is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
        size: meta.as_ref().map_or(0, |m| m.len()),
        modified: meta.and_then(|m| m.modified().ok()),
    };

    let single_dir = match args {
        [] => Some("."),
        [p] if std::path::Path::new(p).is_dir() => Some(p.as_str()),
        _ => None,
    };
    let mut items: Vec<Item> = match single_dir {
        Some(path) => match fs::read_dir(path) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|e| item(e.file_name().to_string_lossy().into_owned(), e.metadata().ok()))
                .collect(),
            Err(e) => {
                eprintln!("dir: {path}: {}", error_msg(&e));
                return 1;
            }
        },
        None => {
            let mut found = Vec::new();
            for a in args {
                match fs::metadata(a) {
                    Ok(m) => found.push(item(a.clone(), Some(m))),
                    Err(e) => eprintln!("dir: {a}: {}", error_msg(&e)),
                }
            }
            if found.is_empty() {
                return 1;
            }
            found
        }
    };
    items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));

    if items.is_empty() {
        println!("{GRAY}(carpeta vacía){RESET}");
        return 0;
    }

    let (dirs, files) = items.iter().partition::<Vec<&Item>, _>(|i| i.is_dir);
    let total: u64 = files.iter().map(|i| i.size).sum();
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|i| {
            let modified = i
                .modified
                .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            if i.is_dir {
                vec![format!("{BLUE}{}{}{RESET}", i.name, std::path::MAIN_SEPARATOR), "carpeta".into(), String::new(), modified]
            } else {
                vec![i.name.clone(), "archivo".into(), human_size(i.size), modified]
            }
        })
        .collect();

    table::print(
        &["Nombre", "Tipo", "Tamaño", "Modificado"],
        &[Align::Left, Align::Left, Align::Right, Align::Left],
        &rows,
    );
    println!("{GRAY}{} carpetas · {} archivos · {}{RESET}", dirs.len(), files.len(), human_size(total));
    0
}

/// Mensajes de error del sistema en español; los poco comunes quedan como los da el sistema.
pub fn error_msg(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "no existe".into(),
        io::ErrorKind::PermissionDenied => "acceso denegado".into(),
        io::ErrorKind::NotADirectory => "no es una carpeta".into(),
        _ => e.to_string(),
    }
}

/// 1536 → "1.5 KB"
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn help() -> i32 {
    let rows: Vec<Vec<String>> = [
        ("cd [carpeta]", "Cambia de carpeta (~ = tu carpeta de usuario)"),
        ("z [palabra]", "Salta a una carpeta frecuente (z crm); sin nada, selector"),
        ("dir [carpeta]", "Lista el contenido de una carpeta (también: ls)"),
        ("cls", "Limpia la pantalla (también: clear)"),
        ("pwd", "Muestra la carpeta actual"),
        ("mkdir <carpeta>", "Crea carpetas"),
        ("touch <archivo>", "Crea un archivo vacío (o actualiza su fecha)"),
        ("cat <archivo>", "Muestra el contenido de un archivo"),
        ("cp <origen> <destino>", "Copia archivos o carpetas"),
        ("mv <origen> <destino>", "Mueve o renombra archivos o carpetas"),
        ("rm <archivo>", "Manda archivos o carpetas a la papelera"),
        ("open [ruta]", "Abre la carpeta en el Explorador (o un archivo/URL)"),
        ("echo [texto]", "Imprime texto"),
        ("set [NOMBRE=valor]", "Define una variable solo para esta sesión; sin argumentos, las lista"),
        ("env", "Variables de entorno permanentes: ver, crear, editar o borrar"),
        ("path", "Carpetas del PATH: agregar, editar, quitar u ordenar"),
        ("history [texto]", "Comandos usados, sin repetir; el elegido queda escrito en la línea"),
        ("alias [n = comando]", "Atajos: sin nada los lista; con nombre = comando crea uno"),
        ("unalias <nombre>", "Borra un alias"),
        ("config", "Edita ~/.fyshrc (se ejecuta al abrir fysh) y lo recarga"),
        ("ssh", "Servidores: conectar, agregar, editar, borrar, subir y bajar archivos"),
        ("tunel [nombre]", "Túneles SSH (ej. MySQL de AWS por el bastión): crear, activar, apagar"),
        ("db [nombre]", "Conexiones MySQL/PostgreSQL: consola, ver tablas y filas"),
        ("run [script]", "Scripts del proyecto: npm, Django, docker-compose, Makefile, cargo"),
        ("http [MÉTODO] url …", "Probar APIs: http :8000/api/leads · POST … nombre=Ana edad:=30"),
        ("puertos [n]", "Qué programa usa cada puerto; abrirlo o cerrarlo (--todos)"),
        ("procesos [texto]", "Procesos que más memoria usan; cerrar uno"),
        ("docker", "Contenedores: logs, entrar, reiniciar, detener, iniciar, borrar"),
        ("agent", "Elige qué IA instalada es tu asistente (para explica e ins:)"),
        ("explica", "El asistente explica el último error y sugiere cómo arreglarlo"),
        ("ins: <tarea>", "El asistente escribe el comando para una tarea (ins: borrar rama x)"),
        ("actualizar", "Instala la última versión de fysh (también: update)"),
        ("help", "Muestra esta ayuda"),
        ("exit [código]", "Cierra fysh"),
    ]
    .iter()
    .map(|(c, d)| vec![c.to_string(), d.to_string()])
    .collect();
    table::print(&["Comando", "Descripción"], &[Align::Left, Align::Left], &rows);
    println!("{GRAY}Variables: $NOMBRE usa una variable, $? es el código del último comando.{RESET}");
    println!("{GRAY}Cualquier otro comando se busca en el PATH (git, notepad, ping...).{RESET}");
    println!("{GRAY}Combinar: a | b (pipe) · a > f, a >> f, a 2> f, a 2>&1, a < f · a && b, a || b, a ; b · *.txt{RESET}");
    println!("{GRAY}Teclas: Tab completa · ↑↓ historial · Ctrl+R buscar en el historial · Ctrl+T buscar un archivo{RESET}");
    println!("{GRAY}Proyectos: al entrar se activa su .venv y se ofrece cargar su .env. Comandos de más de 30 s{RESET}");
    println!("{GRAY}avisan al terminar si no estás mirando (set FYSH_AVISO_SEGUNDOS=60 para cambiarlo, 0 lo apaga).{RESET}");
    println!("{GRAY}Git: status, log y branch se muestran en tablas; switch, checkout, add, show y commit{RESET}");
    println!("{GRAY}sin argumentos abren un selector. Con argumentos, git funciona como siempre.{RESET}");
    println!("{GRAY}Agentes de IA: claude (Claude Code), agy o antigravity (Antigravity) y opencode (OpenCode){RESET}");
    println!("{GRAY}muestran tus conversaciones de esta carpeta para reanudar una o empezar otra.{RESET}");
    println!("{GRAY}agy -rename / agy -delete: renombrar o borrar una conversación de Antigravity.{RESET}");
    0
}

#[cfg(test)]
mod tests {
    use super::human_size;

    #[test]
    fn tamanos() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }
}
