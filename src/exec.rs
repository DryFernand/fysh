//! Ejecuta una línea ya entendida por el parser: `&&`, `||`, `;`, pipes y redirecciones.
//!
//! Un comando solo y sin redirecciones corre dentro de fysh, como siempre (selectores, cd, alias…).
//! En un pipe o con redirecciones, cada etapa corre como `fysh --exec <args>`: así los comandos
//! propios (dir, git status en tabla…) también funcionan, y su salida pasa por un filtro que
//! quita los colores cuando va a un archivo o a otro programa.

use crate::parser::{self, Op, Pipeline, Sink};
use crate::{builtins, run_external, Shell};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Ejecuta la línea completa y devuelve el código del último comando que corrió.
pub fn run_line(shell: &mut Shell, line: &str) -> i32 {
    let parsed = match parser::parse_line(line, &shell.aliases) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let mut status = 0;
    for (op, pipeline) in parsed {
        match op {
            Op::And if status != 0 => continue,
            Op::Or if status == 0 => continue,
            _ => {}
        }
        status = run_pipeline(shell, &pipeline);
        shell.last_status = status; // el siguiente comando ve este $?
        if !shell.running {
            break;
        }
    }
    status
}

/// Ejecuta un comando sin pipes ni redirecciones dentro de fysh (o el programa externo).
pub fn run_args(shell: &mut Shell, args: &[String]) -> i32 {
    if args.is_empty() {
        return shell.last_status;
    }
    builtins::run(args, shell).unwrap_or_else(|| run_external(args))
}

fn run_pipeline(shell: &mut Shell, cmds: &Pipeline) -> i32 {
    let expanded: Vec<Vec<String>> = cmds.iter().map(|c| parser::expand_args(&c.words, shell)).collect();
    if let ([cmd], [args]) = (cmds.as_slice(), expanded.as_slice()) {
        if !cmd.has_redirects() {
            return run_args(shell, args);
        }
    }
    match spawn_pipeline(shell, cmds, &expanded) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fysh: {e}");
            1
        }
    }
}

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// A dónde va la salida de una etapa.
enum Target {
    Terminal,
    Writer(SharedWriter),
}

fn open_sink(path: &str, append: bool) -> std::io::Result<File> {
    if append {
        OpenOptions::new().create(true).append(true).open(path)
    } else {
        File::create(path)
    }
}

fn spawn_pipeline(shell: &Shell, cmds: &Pipeline, expanded: &[Vec<String>]) -> std::io::Result<i32> {
    let exe = std::env::current_exe()?;
    let mut children: Vec<Child> = Vec::new();
    let mut pumps: Vec<JoinHandle<()>> = Vec::new();
    let mut next_stdin: Option<os_pipe::PipeReader> = None;
    let last = cmds.len() - 1;

    for (i, (cmd, args)) in cmds.iter().zip(expanded).enumerate() {
        // Entrada: archivo con <, la etapa anterior, o la terminal.
        let stdin = match &cmd.stdin {
            Some(w) => {
                let path = parser::expand_word(w, shell);
                Stdio::from(File::open(&path).map_err(|e| std::io::Error::new(e.kind(), format!("{path}: {e}")))?)
            }
            None => next_stdin.take().map_or(Stdio::inherit(), Stdio::from),
        };

        // Salida: archivo con > / >>, la siguiente etapa, o la terminal.
        let mut pipe_reader = None;
        let stdout_target = match &cmd.stdout {
            Some(Sink::File { path, append }) => {
                let path = parser::expand_word(path, shell);
                let file = open_sink(&path, *append).map_err(|e| std::io::Error::new(e.kind(), format!("{path}: {e}")))?;
                Target::Writer(Arc::new(Mutex::new(Box::new(file))))
            }
            _ if i < last => {
                let (reader, writer) = os_pipe::pipe()?;
                pipe_reader = Some(reader);
                Target::Writer(Arc::new(Mutex::new(Box::new(writer))))
            }
            _ => Target::Terminal,
        };
        // Si la salida se fue a un archivo, la etapa siguiente recibe entrada vacía (como bash).
        if i < last && pipe_reader.is_none() {
            let (reader, writer) = os_pipe::pipe()?;
            drop(writer);
            pipe_reader = Some(reader);
        }

        // Errores: archivo con 2> / 2>>, a donde va la salida con 2>&1, o la terminal.
        let stderr_target = match &cmd.stderr {
            Some(Sink::File { path, append }) => {
                let path = parser::expand_word(path, shell);
                let file = open_sink(&path, *append).map_err(|e| std::io::Error::new(e.kind(), format!("{path}: {e}")))?;
                Target::Writer(Arc::new(Mutex::new(Box::new(file))))
            }
            Some(Sink::Stdout) => match &stdout_target {
                Target::Terminal => Target::Terminal,
                Target::Writer(w) => Target::Writer(w.clone()),
            },
            None => Target::Terminal,
        };

        let mut child_cmd = Command::new(&exe);
        child_cmd.arg("--exec").args(args).stdin(stdin);
        child_cmd.stdout(if matches!(stdout_target, Target::Terminal) { Stdio::inherit() } else { Stdio::piped() });
        child_cmd.stderr(if matches!(stderr_target, Target::Terminal) { Stdio::inherit() } else { Stdio::piped() });
        let mut child = child_cmd.spawn()?;

        if let Target::Writer(w) = stdout_target {
            if let Some(out) = child.stdout.take() {
                pumps.push(pump(out, w));
            }
        }
        if let Target::Writer(w) = stderr_target {
            if let Some(err) = child.stderr.take() {
                pumps.push(pump(err, w));
            }
        }
        children.push(child);
        next_stdin = pipe_reader;
    }
    drop(next_stdin);

    let mut code = 0;
    for mut child in children {
        code = child.wait().map(|s| s.code().unwrap_or(1)).unwrap_or(1);
    }
    for p in pumps {
        let _ = p.join();
    }
    Ok(code)
}

/// Copia la salida de un proceso a su destino quitando los colores ANSI.
fn pump(mut from: impl Read + Send + 'static, to: SharedWriter) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut strip = AnsiStripper::default();
        let mut buf = [0u8; 8192];
        let mut out = Vec::new();
        loop {
            let n = match from.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            out.clear();
            strip.feed(&buf[..n], &mut out);
            let Ok(mut w) = to.lock() else { break };
            if w.write_all(&out).is_err() {
                break; // el siguiente programa terminó antes de leer todo (ej: `| more` cerrado)
            }
        }
        if let Ok(mut w) = to.lock() {
            let _ = w.flush();
        }
    })
}

/// Quita secuencias ANSI de un flujo de bytes que llega en pedazos: colores y movimientos
/// de cursor (ESC [ ... letra) y títulos (ESC ] ... BEL).
#[derive(Default)]
pub struct AnsiStripper {
    state: State,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum State {
    #[default]
    Text,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

impl AnsiStripper {
    pub fn feed(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &b in input {
            self.state = match (self.state, b) {
                (State::Text, 0x1b) => State::Esc,
                (State::Text, _) => {
                    out.push(b);
                    State::Text
                }
                (State::Esc, b'[') => State::Csi,
                (State::Esc, b']') => State::Osc,
                (State::Esc, _) => State::Text, // ESC + una letra: se descarta entera
                (State::Csi, 0x40..=0x7e) => State::Text,
                (State::Csi, _) => State::Csi,
                (State::Osc, 0x07) => State::Text,
                (State::Osc, 0x1b) => State::OscEsc,
                (State::Osc, _) => State::Osc,
                (State::OscEsc, b'\\') => State::Text,
                (State::OscEsc, _) => State::Osc,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(chunks: &[&[u8]]) -> String {
        let mut s = AnsiStripper::default();
        let mut out = Vec::new();
        for c in chunks {
            s.feed(c, &mut out);
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn quita_colores_aunque_lleguen_partidos() {
        assert_eq!(strip(&[b"\x1b[38;2;255;70;70mrojo\x1b[0m normal"]), "rojo normal");
        assert_eq!(strip(&[b"a\x1b[38;2", b";1;2;3mb\x1b", b"[0mc"]), "abc");
        assert_eq!(strip(&[b"\x1b[2J\x1b[3J\x1b[Hlimpio"]), "limpio", "no solo colores: también cls");
        assert_eq!(strip(&[b"\x1b]0;fysh\x07titulo"]), "titulo");
        assert_eq!(strip(&["│ tabla ✓".as_bytes()]), "│ tabla ✓");
    }
}
