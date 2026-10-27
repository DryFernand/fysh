mod builtins;
mod parser;

use std::io::{self, Write};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Se pone en true cuando el usuario presiona Ctrl+C.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

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
    println!("fysh {} - escribe 'help' para ver los comandos\n", env!("CARGO_PKG_VERSION"));

    while shell.running {
        INTERRUPTED.store(false, Ordering::SeqCst);
        print_prompt();

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

fn print_prompt() {
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    print!("{cwd} > ");
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
