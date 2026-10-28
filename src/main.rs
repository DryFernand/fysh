mod agents;
mod ai;
mod aliases;
mod ask;
mod builtins;
mod complete;
mod envvars;
mod files;
mod git;
mod history;
mod jump;
mod parser;
mod picker;
mod ssh;
mod table;

use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{CompletionType, Config, Editor};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

// Colores ANSI. Cambia los números para cambiar el estilo (38;2;R;G;B = color RGB).
pub const RED: &str = "\x1b[38;2;255;70;70m";
pub const GRAY: &str = "\x1b[38;2;140;145;155m";
pub const BLUE: &str = "\x1b[38;2;97;175;239m";
pub const GREEN: &str = "\x1b[38;2;80;250;123m";
pub const YELLOW: &str = "\x1b[38;2;241;196;15m";
pub const BOLD: &str = "\x1b[1m";
pub const RESET: &str = "\x1b[0m";

const BANNER: &str = r"
  ███████╗██╗   ██╗███████╗██╗  ██╗
  ██╔════╝╚██╗ ██╔╝██╔════╝██║  ██║
  █████╗   ╚████╔╝ ███████╗███████║
  ██╔══╝    ╚██╔╝  ╚════██║██╔══██║
  ██║        ██║   ███████║██║  ██║
  ╚═╝        ╚═╝   ╚══════╝╚═╝  ╚═╝";

/// Desde cuánto tarda un comando para mostrar su duración en el prompt.
const SLOW: Duration = Duration::from_secs(5);

/// Un comando que falló, para que `explica` se lo pase al asistente.
pub struct Failure {
    pub command: String,
    pub code: i32,
    /// Lo que se vio en pantalla mientras corría (solo en Windows).
    pub output: Option<String>,
}

/// Estado compartido de la shell. Agrega campos aquí.
pub struct Shell {
    pub last_status: i32,
    pub running: bool,
    /// alias → comando (ordenados por nombre).
    pub aliases: std::collections::BTreeMap<String, String>,
    /// Línea tal como se escribió (alias la necesita para no perder comillas).
    pub line: String,
    /// true mientras se ejecuta ~/.fyshrc al abrir fysh.
    pub loading_rc: bool,
    /// Texto que aparece ya escrito en la siguiente línea (history, explica, ins:).
    pub pending: Option<String>,
    /// Cuánto tardó el último comando.
    pub last_duration: Duration,
    pub last_failure: Option<Failure>,
}

impl Shell {
    pub fn new() -> Self {
        Shell {
            last_status: 0,
            running: true,
            aliases: Default::default(),
            line: String::new(),
            loading_rc: false,
            pending: None,
            last_duration: Duration::ZERO,
            last_failure: None,
        }
    }
}

/// Ejecuta una línea: expande alias y corre el comando interno o el programa externo.
pub fn execute(shell: &mut Shell, line: &str) {
    let args = parser::parse(line, shell);
    if args.is_empty() {
        return;
    }
    let args = aliases::expand(shell, args);
    shell.line = line.to_string();
    let start_row = ai::console_row();
    let started = Instant::now();
    shell.last_status = match builtins::run(&args, shell) {
        Some(code) => code,
        None => run_external(&args),
    };
    shell.last_duration = started.elapsed();
    // Los comandos de IA no se guardan como "último error": explica debe ver el comando anterior.
    if shell.last_status != 0 && !["explica", "agent"].contains(&args[0].as_str()) && !args[0].starts_with("ins:") {
        shell.last_failure = Some(Failure {
            command: line.trim().to_string(),
            code: shell.last_status,
            output: start_row.and_then(ai::console_text_since),
        });
    }
}

fn main() {
    let mut shell = Shell::new();

    // Ctrl+C mientras corre un programa lo corta a él, no a la shell.
    ctrlc::set_handler(|| {}).ok();

    print!("\x1b]0;fysh\x07"); // título de la ventana
    println!("{RED}{BANNER}{RESET}");
    println!("  {GRAY}v{} · escribe 'help' para ver los comandos{RESET}\n", env!("CARGO_PKG_VERSION"));
    aliases::load(&mut shell);

    // Editor de línea: Tab completa, ↑↓ recorren el historial, las sugerencias grises salen del historial.
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .auto_add_history(true)
        .max_history_size(1000)
        .map(|b| b.build());
    let mut editor = match config.and_then(Editor::<complete::FyshHelper, FileHistory>::with_config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("fysh: no se pudo iniciar el editor de línea: {e}");
            std::process::exit(1);
        }
    };
    editor.set_helper(Some(complete::FyshHelper::new()));
    if let Some(helper) = editor.helper() {
        *helper.aliases.borrow_mut() = shell.aliases.keys().cloned().collect();
    }
    let history_file = history_path();
    if let Some(h) = &history_file {
        editor.load_history(h).ok();
    }
    let mut last_dir = std::env::current_dir().ok();
    if let Some(d) = &last_dir {
        jump::visit(d);
    }

    while shell.running {
        let colored = prompt(&shell);
        if let Some(helper) = editor.helper() {
            *helper.colored_prompt.borrow_mut() = colored.clone();
        }
        let plain = table::strip_ansi(&colored);
        let read = match shell.pending.take() {
            Some(cmd) => editor.readline_with_initial(&plain, (&cmd, "")),
            None => editor.readline(&plain),
        };
        let line = match read {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) => continue, // Ctrl+C en el prompt: línea nueva
            Err(_) => break,                             // Ctrl+D / Ctrl+Z o fin de entrada
        };
        if let Some(h) = &history_file {
            editor.save_history(h).ok();
        }

        // `history [texto]` vive aquí porque necesita el historial del editor de línea.
        let args = parser::parse(&line, &shell);
        if args.first().is_some_and(|a| a == "history") && args.len() <= 2 {
            let entries = editor.history().iter().map(String::as_str);
            shell.pending = history::pick(entries, args.get(1).map(String::as_str));
            shell.last_status = 0;
            shell.last_duration = Duration::ZERO;
            continue;
        }

        execute(&mut shell, &line);
        if let Some(helper) = editor.helper() {
            *helper.aliases.borrow_mut() = shell.aliases.keys().cloned().collect();
        }
        // Registrar la carpeta si cambió, para `z`.
        let dir = std::env::current_dir().ok();
        if dir != last_dir {
            if let Some(d) = &dir {
                jump::visit(d);
            }
            last_dir = dir;
        }
    }
    std::process::exit(shell.last_status);
}

/// Historial guardado entre sesiones: ~/.fysh_history
fn history_path() -> Option<PathBuf> {
    home().map(|h| h.join(".fysh_history"))
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

/// Ruta con la carpeta de usuario abreviada como ~
pub fn tilde(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    match home().map(|h| h.display().to_string()) {
        Some(h) if !h.is_empty() && text.starts_with(&h) => format!("~{}", &text[h.len()..]),
        _ => text,
    }
}

/// Prompt: "fysh ~\carpeta (rama ●3) ⏱ 12s [1] ❯". La duración sale si el comando tardó más de 5 s;
/// el código, si falló.
fn prompt(shell: &Shell) -> String {
    let cwd = std::env::current_dir().map(|p| tilde(&p)).unwrap_or_default();
    let took = if shell.last_duration >= SLOW {
        format!("{YELLOW}⏱ {}{RESET} ", format_duration(shell.last_duration))
    } else {
        String::new()
    };
    let status = match shell.last_status {
        0 => String::new(),
        code => format!("{RED}[{code}] "),
    };
    let git = git::prompt_segment();
    format!("{BOLD}{RED}fysh{RESET} {GRAY}{cwd}{RESET}{git} {took}{status}{RED}❯{RESET} ")
}

/// 12s · 2m 05s · 1h 03m
fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// Lanza un programa externo y espera a que termine.
pub fn run_external(args: &[String]) -> i32 {
    let program = find_program(&args[0]).unwrap_or_else(|| args[0].clone().into());
    match Command::new(&program).args(&args[1..]).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(_) => {
            eprintln!("fysh: comando no encontrado: {}", args[0]);
            if let Some(cmd) = install_hint(&args[0]) {
                eprintln!("{GRAY}Para instalarlo: {cmd}{RESET}");
            }
            127
        }
    }
}

/// Ruta completa de un programa del PATH, o None si no está instalado.
pub fn which(name: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        return find_program(name);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

/// En Windows busca el programa en el PATH probando .exe, .com, .cmd y .bat.
/// Rust solo prueba .exe, y muchas herramientas (npm, code, agentes de IA) son .cmd.
#[cfg(windows)]
fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::path::Path::new(name);
    if path.extension().is_some() || path.components().count() > 1 {
        return None; // ya trae extensión o ruta: se usa tal cual
    }
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let exts: Vec<&str> = exts
        .split(';')
        .filter(|e| [".com", ".exe", ".bat", ".cmd"].contains(&e.to_lowercase().as_str()))
        .collect();
    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn find_program(_name: &str) -> Option<PathBuf> {
    None // en Linux/Mac el sistema ya busca en el PATH
}

/// Cómo instalar los agentes de IA conocidos, si no están.
pub fn install_hint(name: &str) -> Option<&'static str> {
    match name {
        "claude" if cfg!(windows) => Some("irm https://claude.ai/install.ps1 | iex"),
        "claude" => Some("curl -fsSL https://claude.ai/install.sh | bash"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatea_duraciones() {
        assert_eq!(format_duration(Duration::from_secs(12)), "12s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(format_duration(Duration::from_secs(3780)), "1h 03m");
    }
}
