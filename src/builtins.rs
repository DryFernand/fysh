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
        "claude" if rest.is_empty() => crate::agents::claude_picker(),
        "agy" | "antigravity" if rest.is_empty() => crate::agents::agy_picker(),
        "opencode" => crate::agents::opencode(rest),
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
fn dir(args: &[String]) -> i32 {
    let path = args.first().map(String::as_str).unwrap_or(".");
    let entries = match fs::read_dir(path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("dir: {path}: {}", error_msg(&e));
            return 1;
        }
    };

    struct Item {
        name: String,
        is_dir: bool,
        size: u64,
        modified: Option<std::time::SystemTime>,
    }
    let mut items: Vec<Item> = entries
        .filter_map(Result::ok)
        .map(|e| {
            let meta = e.metadata().ok();
            Item {
                name: e.file_name().to_string_lossy().into_owned(),
                is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                size: meta.as_ref().map_or(0, |m| m.len()),
                modified: meta.and_then(|m| m.modified().ok()),
            }
        })
        .collect();
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
fn error_msg(e: &io::Error) -> String {
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
        ("dir [carpeta]", "Lista el contenido de una carpeta (también: ls)"),
        ("cls", "Limpia la pantalla (también: clear)"),
        ("pwd", "Muestra la carpeta actual"),
        ("echo [texto]", "Imprime texto"),
        ("set [NOMBRE=valor]", "Define una variable; sin argumentos, las lista"),
        ("help", "Muestra esta ayuda"),
        ("exit [código]", "Cierra fysh"),
    ]
    .iter()
    .map(|(c, d)| vec![c.to_string(), d.to_string()])
    .collect();
    table::print(&["Comando", "Descripción"], &[Align::Left, Align::Left], &rows);
    println!("{GRAY}Variables: $NOMBRE usa una variable, $? es el código del último comando.{RESET}");
    println!("{GRAY}Cualquier otro comando se busca en el PATH (git, notepad, ping...).{RESET}");
    println!("{GRAY}Agentes de IA: claude (Claude Code), agy o antigravity (Antigravity) y opencode (OpenCode){RESET}");
    println!("{GRAY}muestran tus conversaciones de esta carpeta para reanudar una o empezar otra.{RESET}");
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
