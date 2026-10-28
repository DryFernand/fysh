mod agents;
mod builtins;
mod complete;
mod envvars;
mod git;
mod parser;
mod picker;
mod table;

use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{CompletionType, Config, Editor};
use std::path::PathBuf;
use std::process::Command;

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

/// Estado compartido de la shell. Agrega campos aquí (aliases, historial, etc.).
pub struct Shell {
    pub last_status: i32,
    pub running: bool,
}

fn main() {
    let mut shell = Shell { last_status: 0, running: true };

    // Ctrl+C mientras corre un programa lo corta a él, no a la shell.
    ctrlc::set_handler(|| {}).ok();

    print!("\x1b]0;fysh\x07"); // título de la ventana
    println!("{RED}{BANNER}{RESET}");
    println!("  {GRAY}v{} · escribe 'help' para ver los comandos{RESET}\n", env!("CARGO_PKG_VERSION"));

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
    let history = history_path();
    if let Some(h) = &history {
        editor.load_history(h).ok();
    }

    while shell.running {
        let colored = prompt(&shell);
        if let Some(helper) = editor.helper() {
            *helper.colored_prompt.borrow_mut() = colored.clone();
        }
        let line = match editor.readline(&table::strip_ansi(&colored)) {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) => continue, // Ctrl+C en el prompt: línea nueva
            Err(_) => break,                             // Ctrl+D / Ctrl+Z o fin de entrada
        };
        if let Some(h) = &history {
            editor.save_history(h).ok();
        }

        let args = parser::parse(&line, &shell);
        if args.is_empty() {
            continue;
        }

        shell.last_status = match builtins::run(&args, &mut shell) {
            Some(code) => code,
            None => run_external(&args),
        };
    }
    std::process::exit(shell.last_status);
}

/// Historial guardado entre sesiones: ~/.fysh_history
fn history_path() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| PathBuf::from(h).join(".fysh_history"))
}

/// Prompt: "fysh ~\carpeta (rama ●3) ❯". Si el último comando falló, muestra su código.
fn prompt(shell: &Shell) -> String {
    let mut cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    if !home.is_empty() && cwd.starts_with(&home) {
        cwd = format!("~{}", &cwd[home.len()..]);
    }
    let status = match shell.last_status {
        0 => String::new(),
        code => format!("{RED}[{code}] "),
    };
    let git = git::prompt_segment();
    format!("{BOLD}{RED}fysh{RESET} {GRAY}{cwd}{RESET}{git} {status}{RED}❯{RESET} ")
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

/// En Windows busca el programa en el PATH probando .exe, .com, .cmd y .bat.
/// Rust solo prueba .exe, y muchas herramientas (npm, code, agentes de IA) son .cmd.
#[cfg(windows)]
fn find_program(name: &str) -> Option<std::path::PathBuf> {
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
fn find_program(_name: &str) -> Option<std::path::PathBuf> {
    None // en Linux/Mac el sistema ya busca en el PATH
}

/// Cómo instalar los agentes de IA conocidos, si no están.
fn install_hint(name: &str) -> Option<&'static str> {
    match name {
        "claude" if cfg!(windows) => Some("irm https://claude.ai/install.ps1 | iex"),
        "claude" => Some("curl -fsSL https://claude.ai/install.sh | bash"),
        _ => None,
    }
}
