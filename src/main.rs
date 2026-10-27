mod builtins;
mod parser;
mod table;

use std::io::{self, Write};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Se pone en true cuando el usuario presiona Ctrl+C.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

// Colores ANSI. Cambia los números para cambiar el estilo (38;2;R;G;B = color RGB).
pub const RED: &str = "\x1b[38;2;255;70;70m";
pub const GRAY: &str = "\x1b[38;2;140;145;155m";
pub const BLUE: &str = "\x1b[38;2;97;175;239m";
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

    // Ctrl+C mata al programa que esté corriendo, no a la shell.
    ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst)).ok();

    print!("\x1b]0;fysh\x07"); // título de la ventana
    println!("{RED}{BANNER}{RESET}");
    println!("  {GRAY}v{} · escribe 'help' para ver los comandos{RESET}\n", env!("CARGO_PKG_VERSION"));

    while shell.running {
        INTERRUPTED.store(false, Ordering::SeqCst);
        print_prompt(&shell);

        let mut line = String::new();
        let read = io::stdin().read_line(&mut line);
        if matches!(read, Ok(0) | Err(_)) {
            // En Windows, Ctrl+C en el prompt corta la lectura igual que un EOF;
            // el handler corre en otro hilo, así que le damos un instante.
            // ponytail: espera fija de 50ms; si falla en PCs lentas, usar ReadConsoleW directo.
            std::thread::sleep(Duration::from_millis(50));
            if INTERRUPTED.load(Ordering::SeqCst) {
                println!();
                continue;
            }
            break; // EOF real (Ctrl+Z en Windows, Ctrl+D en Unix)
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

/// Prompt: "fysh ~\carpeta ❯". Si el último comando falló, muestra su código.
fn print_prompt(shell: &Shell) {
    let mut cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    if !home.is_empty() && cwd.starts_with(&home) {
        cwd = format!("~{}", &cwd[home.len()..]);
    }
    let status = match shell.last_status {
        0 => String::new(),
        code => format!("{RED}[{code}] "),
    };
    print!("{BOLD}{RED}fysh{RESET} {GRAY}{cwd}{RESET} {status}{RED}❯{RESET} ");
    io::stdout().flush().ok();
}

/// Lanza un programa externo y espera a que termine.
fn run_external(args: &[String]) -> i32 {
    match Command::new(&args[0]).args(&args[1..]).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(_) => {
            eprintln!("fysh: comando no encontrado: {}", args[0]);
            127
        }
    }
}
