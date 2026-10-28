mod agents;
mod ai;
mod aliases;
mod ask;
mod builtins;
mod complete;
mod envvars;
mod exec;
mod files;
mod git;
mod history;
mod jump;
mod parser;
mod picker;
mod ssh;
mod table;
mod update;

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

/// Ejecuta una línea: alias, &&, ||, ;, pipes, redirecciones, comandos propios y programas.
pub fn execute(shell: &mut Shell, line: &str) {
    // BOM: marca invisible que agregan PowerShell y Notepad al inicio del texto.
    let line = line.trim_start_matches('\u{feff}');
    let first = parser::parse(line, shell);
    let Some(cmd) = first.first().cloned() else { return };
    shell.line = line.to_string();
    let start_row = ai::console_row();
    let started = Instant::now();
    // `alias` e `ins:` usan el resto de la línea tal cual: su texto puede llevar | && ; sin ser operadores.
    shell.last_status = if cmd == "alias" || cmd.starts_with("ins:") {
        exec::run_args(shell, &first)
    } else {
        exec::run_line(shell, line)
    };
    shell.last_duration = started.elapsed();
    // Los comandos de IA no se guardan como "último error": explica debe ver el comando anterior.
    if shell.last_status != 0 && !["explica", "agent"].contains(&cmd.as_str()) && !cmd.starts_with("ins:") {
        shell.last_failure = Some(Failure {
            command: line.trim().to_string(),
            code: shell.last_status,
            output: start_row.and_then(ai::console_text_since),
        });
    }
}

/// Nombre del servidor si fysh corre dentro de una conexión ssh, para el prompt "fysh@Prueba".
/// `ssh` en fysh pasa el nombre guardado en FYSH_HOST; si se abrió a mano por ssh, el nombre del equipo.
fn remote_label(fysh_host: Option<String>, over_ssh: bool, hostname: Option<String>) -> Option<String> {
    let clean = |s: String| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    fysh_host.and_then(clean).or_else(|| if over_ssh { hostname.and_then(clean).or(Some("servidor".into())) } else { None })
}

static REMOTE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

pub fn remote() -> Option<&'static str> {
    REMOTE
        .get_or_init(|| {
            let over_ssh = std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
            let hostname = std::env::var("HOSTNAME").ok().or_else(|| std::fs::read_to_string("/etc/hostname").ok());
            remote_label(std::env::var("FYSH_HOST").ok(), over_ssh, hostname)
        })
        .as_deref()
}

/// Título de la pestaña de la terminal.
pub fn set_title(title: &str) {
    print!("\x1b]0;{title}\x07");
    std::io::Write::flush(&mut std::io::stdout()).ok();
}

/// "fysh" en tu PC, "fysh@Prueba" dentro de un servidor.
pub fn shell_name() -> String {
    match remote() {
        Some(host) => format!("fysh@{host}"),
        None => "fysh".into(),
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    match argv.get(1).map(String::as_str) {
        Some("--version") => {
            println!("fysh {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        // `fysh -c "dir | findstr rs"`: ejecuta una línea y sale.
        Some("-c") => {
            let mut shell = Shell::new();
            aliases::load(&mut shell);
            execute(&mut shell, argv.get(2).map_or("", String::as_str));
            std::process::exit(shell.last_status);
        }
        // Uso interno: una etapa de un pipe (argumentos ya expandidos por el fysh que lo lanzó).
        Some("--exec") => {
            let mut shell = Shell::new();
            let code = exec::run_args(&mut shell, &argv[2..]);
            std::process::exit(code);
        }
        _ => {}
    }
    let mut shell = Shell::new();

    // Ctrl+C mientras corre un programa lo corta a él, no a la shell.
    ctrlc::set_handler(|| {}).ok();

    set_title(&shell_name());
    println!("{RED}{BANNER}{RESET}");
    let place = remote().map(|h| format!(" · en {YELLOW}{h}{GRAY} (por ssh)")).unwrap_or_default();
    println!("  {GRAY}v{}{place} · escribe 'help' para ver los comandos{RESET}\n", env!("CARGO_PKG_VERSION"));
    if let Some(notice) = update::notice() {
        println!("{notice}");
    }
    update::check_in_background();
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
    // Dentro de un servidor el nombre va en amarillo, para no confundirlo con tu PC.
    let name = match remote() {
        Some(_) => format!("{BOLD}{YELLOW}{}{RESET}", shell_name()),
        None => format!("{BOLD}{RED}fysh{RESET}"),
    };
    format!("{name} {GRAY}{cwd}{RESET}{git} {took}{status}{RED}❯{RESET} ")
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
    fn nombre_del_servidor_en_el_prompt() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(remote_label(s("Prueba"), true, s("abc123")), s("Prueba")); // nombre guardado gana
        assert_eq!(remote_label(None, true, s("ip-172-31-5-9\n")), s("ip-172-31-5-9"));
        assert_eq!(remote_label(None, true, None), s("servidor"));
        assert_eq!(remote_label(None, false, s("mi-pc")), None); // en tu PC: sin @
        assert_eq!(remote_label(s("  "), false, None), None);
    }

    #[test]
    fn formatea_duraciones() {
        assert_eq!(format_duration(Duration::from_secs(12)), "12s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(format_duration(Duration::from_secs(3780)), "1h 03m");
    }
}
