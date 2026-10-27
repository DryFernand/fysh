use crate::Shell;
use std::env;

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
        "help" => help(),
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
            eprintln!("cd: {target}: {e}");
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

/// set NOMBRE=valor  → define variable de entorno. Sin argumentos, las lista.
fn set(args: &[String]) -> i32 {
    if args.is_empty() {
        for (k, v) in env::vars() {
            println!("{k}={v}");
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

fn help() -> i32 {
    println!("fysh - comandos internos:");
    println!("  cd [dir]        cambiar directorio");
    println!("  pwd             mostrar directorio actual");
    println!("  echo [texto]    imprimir texto");
    println!("  set [K=V]       ver/definir variables ($K para usarlas, $? = último código)");
    println!("  help            esta ayuda");
    println!("  exit [código]   salir");
    0
}
