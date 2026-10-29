use crate::table::{self, Align};
use crate::{Shell, BLUE, BOLD, GRAY, RED, RESET};
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
        "buscar" => crate::files::buscar(rest),
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
        // claude / agy / opencode -rename | -delete
        cmd @ ("claude" | "agy" | "antigravity" | "opencode") if matches!(rest, [a] if ["-rename", "--rename", "-delete", "--delete"].contains(&a.as_str())) => {
            crate::agents::manage(if cmd == "antigravity" { "agy" } else { cmd }, &rest[0])
        }
        "claude" if rest.is_empty() => crate::agents::claude_picker(),
        "agy" | "antigravity" if rest.is_empty() => crate::agents::agy_picker(),
        "opencode" => crate::agents::opencode(rest),
        "git" => return crate::git::run(rest),
        "source" if !rest.is_empty() => crate::script::run_file(shell, &rest[0], &rest[1..]),
        "test" => test_cmd(rest),
        "true" => 0,
        "false" => 1,
        "math" => math_cmd(rest),
        "read" => read_cmd(rest),
        "break" => {
            shell.flow = crate::script::Flow::Break;
            0
        }
        "continue" => {
            shell.flow = crate::script::Flow::Continue;
            0
        }
        "return" => {
            shell.flow = crate::script::Flow::Return;
            rest.first().and_then(|a| a.parse().ok()).unwrap_or(shell.last_status)
        }
        "jobs" => crate::jobs::jobs_cmd(shell),
        "fg" => crate::jobs::fg_cmd(shell, rest),
        // `kill %1`: trabajos de fysh; `kill 1234` sigue siendo el programa del sistema.
        "kill" if !rest.is_empty() && rest.iter().all(|a| a.starts_with('%')) => crate::jobs::kill_cmd(shell, rest),
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

/// `test`: condiciones para if y while. 0 = verdadero, 1 = falso, 2 = mal escrito.
fn test_cmd(args: &[String]) -> i32 {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match test_eval(&words) {
        Some(true) => 0,
        Some(false) => 1,
        None => {
            eprintln!("test: no entiendo '{}'. Ejemplos: test -f archivo · test $x = hola · test $n -gt 3", args.join(" "));
            2
        }
    }
}

fn test_eval(a: &[&str]) -> Option<bool> {
    let num = |s: &str| s.trim().parse::<f64>().ok();
    let path = std::path::Path::new;
    Some(match a {
        [] => false,
        ["!", rest @ ..] => !test_eval(rest)?,
        [s] => !s.is_empty(),
        ["-n", s] => !s.is_empty(),
        ["-z", s] => s.is_empty(),
        ["-e", p] => path(p).exists(),
        ["-f", p] => path(p).is_file(),
        ["-d", p] => path(p).is_dir(),
        [x, "=" | "==", y] => x == y,
        [x, "!=", y] => x != y,
        [x, op, y] => {
            let (x, y) = (num(x)?, num(y)?);
            match *op {
                "-eq" => x == y,
                "-ne" => x != y,
                "-lt" => x < y,
                "-le" => x <= y,
                "-gt" => x > y,
                "-ge" => x >= y,
                _ => return None,
            }
        }
        _ => return None,
    })
}

/// `math 2 + 3 x 4`: + - * (o x) / % y paréntesis. Sin comillas, `*` sería un comodín: usa x o "2 * 3".
fn math_cmd(args: &[String]) -> i32 {
    let text = args.join(" ");
    match math_eval(&text) {
        Some(v) if v.fract() == 0.0 && v.abs() < 1e15 => println!("{}", v as i64),
        Some(v) => println!("{}", (v * 1e6).round() / 1e6),
        None => {
            eprintln!("math: no entiendo '{text}'. Ejemplo: math $n + 1");
            return 1;
        }
    }
    0
}

fn math_eval(text: &str) -> Option<f64> {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut pos = 0;
    let value = math_sum(&chars, &mut pos)?;
    (pos == chars.len()).then_some(value)
}

fn math_sum(c: &[char], pos: &mut usize) -> Option<f64> {
    let mut v = math_product(c, pos)?;
    while let Some(&op) = c.get(*pos).filter(|&&ch| ch == '+' || ch == '-') {
        *pos += 1;
        let rhs = math_product(c, pos)?;
        v = if op == '+' { v + rhs } else { v - rhs };
    }
    Some(v)
}

fn math_product(c: &[char], pos: &mut usize) -> Option<f64> {
    let mut v = math_atom(c, pos)?;
    while let Some(&op) = c.get(*pos).filter(|&&ch| matches!(ch, '*' | 'x' | '/' | '%')) {
        *pos += 1;
        let rhs = math_atom(c, pos)?;
        v = match op {
            '/' | '%' if rhs == 0.0 => return None,
            '/' => v / rhs,
            '%' => v % rhs,
            _ => v * rhs,
        };
    }
    Some(v)
}

fn math_atom(c: &[char], pos: &mut usize) -> Option<f64> {
    match c.get(*pos)? {
        '-' => {
            *pos += 1;
            Some(-math_atom(c, pos)?)
        }
        '(' => {
            *pos += 1;
            let v = math_sum(c, pos)?;
            (c.get(*pos) == Some(&')')).then(|| *pos += 1)?;
            Some(v)
        }
        _ => {
            let start = *pos;
            while c.get(*pos).is_some_and(|ch| ch.is_ascii_digit() || *ch == '.') {
                *pos += 1;
            }
            c[start..*pos].iter().collect::<String>().parse().ok()
        }
    }
}

/// `read NOMBRE [pregunta]`: lee una línea del teclado (o de un pipe) y la guarda en $NOMBRE.
fn read_cmd(args: &[String]) -> i32 {
    let Some(name) = args.first() else {
        eprintln!("uso: read NOMBRE [pregunta]");
        return 2;
    };
    if args.len() > 1 {
        print!("{} ", args[1..].join(" "));
        io::stdout().flush().ok();
    }
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => 1,
        Ok(_) => {
            env::set_var(name, line.trim_end_matches(['\r', '\n']));
            0
        }
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
        ("buscar <texto> [-file|-folder]", "Busca por nombre aquí y en todas las subcarpetas; da la ruta completa"),
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
        ("source <archivo>", "Ejecuta un script .fysh en esta sesión (sus funciones quedan definidas)"),
        ("test <condición>", "Para if/while: -f archivo, -d carpeta, -z texto, a = b, n -gt 3, ! niega"),
        ("math <cuenta>", "Calcula: math $n + 1 · math (2 + 3) x 4 (x o \"*\" para multiplicar)"),
        ("read <NOMBRE> [pregunta]", "Pide un texto y lo guarda en $NOMBRE"),
        ("jobs", "Comandos en segundo plano (los que terminaste con &)"),
        ("fg [N]", "Espera a que termine un comando en segundo plano"),
        ("kill %N", "Cierra un comando en segundo plano"),
        ("help", "Muestra esta ayuda"),
        ("exit [código]", "Cierra fysh"),
    ]
    .iter()
    .map(|(c, d)| vec![c.to_string(), d.to_string()])
    .collect();
    table::print(&["Comando", "Descripción"], &[Align::Left, Align::Left], &rows);
    let sections: &[(&str, &[(&str, &str)])] = &[
        ("Escribir comandos", &[
            ("$NOMBRE", "usa una variable · $? es el código del último comando"),
            ("Programas", "cualquier otro comando se busca en el PATH (git, notepad, ping…)"),
            ("a | b", "pasa la salida de a como entrada de b"),
            ("Redirigir", "a > f · a >> f (agregar) · a 2> f (errores) · a 2>&1 · a < f"),
            ("Encadenar", "a && b (si a sale bien) · a || b (si a falla) · a ; b (siempre)"),
            ("*.txt", "comodines: todos los archivos que coinciden"),
            ("$(comando)", "lo que imprime el comando: echo hoy es $(date) · una palabra por línea"),
            ("comando &", "corre en segundo plano y te devuelve el prompt (mira jobs)"),
            ("# texto", "comentario"),
        ]),
        ("Scripts", &[
            ("if", "if test -f .env; echo hay; else; echo no; end"),
            ("for", "for f in *.log; rm $f; end"),
            ("while", "while test $n -lt 5; set n=$(math $n + 1); end"),
            ("function", "function hola; echo hola $1; end   ($1 $2… y $argv)"),
            ("", "break, continue y return dentro de bucles y funciones"),
            ("Varias líneas", "si escribes if/for/while/function, Enter pide la siguiente línea hasta end"),
            ("Archivos", "tarea.fysh: fysh tarea.fysh a b · ./tarea.fysh · source tarea.fysh"),
        ]),
        ("Teclas", &[
            ("Tab", "completa comandos, archivos y ramas"),
            ("↑ ↓", "comandos anteriores"),
            ("Ctrl+R", "buscar en el historial"),
            ("Ctrl+T", "buscar un archivo"),
        ]),
        ("Proyectos", &[
            (".venv", "se activa al entrar a la carpeta"),
            (".env", "al entrar se ofrece cargarlo"),
            ("Avisos", "comandos de más de 30 s avisan al terminar si no estás mirando"),
            ("", "set FYSH_AVISO_SEGUNDOS=60 lo cambia · 0 lo apaga"),
        ]),
        ("Git", &[
            ("Tablas", "git status, git log y git branch"),
            ("Selector", "git switch, checkout, add, show y commit sin argumentos"),
            ("Normal", "con argumentos, git funciona como siempre"),
        ]),
        ("Agentes de IA", &[
            ("claude", "Claude Code"),
            ("agy", "Antigravity (también: antigravity)"),
            ("opencode", "OpenCode"),
            ("", "sin nada: tus conversaciones de esta carpeta, reanudar una o empezar otra"),
            ("-rename", "renombrar una conversación sin abrir el chat (agy -rename)"),
            ("-delete", "borrar una conversación sin abrir el chat (claude -delete)"),
        ]),
    ];
    let width = sections.iter().flat_map(|(_, items)| items.iter()).map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    for (title, items) in sections {
        println!("\n{RED}{BOLD}{title}{RESET}");
        for (key, text) in *items {
            println!("  {BLUE}{key:<width$}{RESET}  {GRAY}{text}{RESET}");
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::{human_size, math_eval, test_eval};

    #[test]
    fn cuentas_y_condiciones() {
        assert_eq!(math_eval("(2 + 3) x 4 - -1"), Some(21.0));
        assert_eq!(math_eval("7 / 2"), Some(3.5));
        assert_eq!(math_eval("10 % 3 * 2"), Some(2.0));
        assert_eq!(math_eval("1 / 0"), None);
        assert_eq!(math_eval("2 +"), None);
        assert_eq!(test_eval(&["3", "-lt", "10"]), Some(true));
        assert_eq!(test_eval(&["!", "a", "=", "a"]), Some(false));
        assert_eq!(test_eval(&["-z", ""]), Some(true));
        assert_eq!(test_eval(&["x", "-gt", "1"]), None);
    }

    #[test]
    fn tamanos() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }
}
