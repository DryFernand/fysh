//! Trabajos en segundo plano: `comando &`, `jobs`, `fg` y `kill %N`.

use crate::parser::{self, Pipeline};
use crate::script::INTERRUPTED;
use crate::table::{self, Align};
use crate::{Shell, GRAY, GREEN, RED, RESET};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

pub struct Job {
    id: usize,
    command: String,
    children: Vec<Child>,
    started: Instant,
}

impl Job {
    /// Código de salida si ya terminaron todas sus etapas.
    fn finished(&mut self) -> Option<i32> {
        let mut code = 0;
        for c in &mut self.children {
            code = match c.try_wait() {
                Ok(Some(s)) => s.code().unwrap_or(1),
                Ok(None) => return None,
                Err(_) => 1,
            };
        }
        Some(code)
    }

    /// Cierra el trabajo con todo lo que lanzó (npm start → node…).
    fn kill(&mut self) {
        for c in &mut self.children {
            let pid = c.id().to_string();
            let mut cmd = if cfg!(windows) {
                let mut k = Command::new("taskkill");
                k.args(["/T", "/F", "/PID", &pid]);
                k
            } else {
                let mut k = Command::new("kill"); // al grupo de procesos que se creó al lanzarlo
                k.args(["-TERM", &format!("-{pid}")]);
                k
            };
            if !cmd.stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success()) {
                let _ = c.kill();
            }
            let _ = c.wait();
        }
    }
}

/// `comando &`: lo lanza sin esperar y lo anota en la lista de trabajos.
pub fn start(shell: &mut Shell, cmds: &Pipeline) -> i32 {
    let expanded: Vec<Vec<String>> = cmds.iter().map(|c| parser::expand_args(&c.words, shell)).collect();
    match crate::exec::spawn_stages(shell, cmds, &expanded, true) {
        Ok((children, _pumps)) => {
            // _pumps: los hilos que copian la salida terminan solos cuando el programa cierra su salida.
            let id = shell.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
            let command = expanded.iter().map(|a| a.join(" ")).collect::<Vec<_>>().join(" | ");
            let pid = children.last().map_or(0, Child::id);
            println!("{GRAY}[{id}] {pid} {command}{RESET}");
            shell.jobs.push(Job { id, command, children, started: Instant::now() });
            0
        }
        Err(e) => {
            eprintln!("fysh: {e}");
            1
        }
    }
}

/// Antes de cada prompt: avisa de los trabajos que terminaron y los quita de la lista.
pub fn reap(shell: &mut Shell) {
    shell.jobs.retain_mut(|job| match job.finished() {
        Some(code) => {
            let (color, mark) = if code == 0 { (GREEN, "✓") } else { (RED, "✗") };
            println!("{color}{mark}{RESET} {GRAY}[{}] terminó (código {code}): {}{RESET}", job.id, job.command);
            false
        }
        None => true,
    });
}

/// `jobs`: tabla de los trabajos que siguen corriendo.
pub fn jobs_cmd(shell: &mut Shell) -> i32 {
    reap(shell);
    if shell.jobs.is_empty() {
        println!("{GRAY}No hay trabajos en segundo plano. Ejemplo: npm run dev &{RESET}");
        return 0;
    }
    let rows: Vec<Vec<String>> = shell
        .jobs
        .iter()
        .map(|j| {
            let pid = j.children.last().map_or(0, Child::id);
            vec![j.id.to_string(), pid.to_string(), crate::format_duration(j.started.elapsed()), j.command.clone()]
        })
        .collect();
    table::print(&["N", "PID", "Corriendo", "Comando"], &[Align::Right, Align::Right, Align::Right, Align::Left], &rows);
    println!("{GRAY}fg N espera a que termine · kill %N lo cierra{RESET}");
    0
}

/// "%2", "2" o nada (el último) → posición en la lista.
fn find(shell: &Shell, arg: Option<&String>, cmd: &str) -> Option<usize> {
    let pos = match arg {
        None => shell.jobs.len().checked_sub(1),
        Some(a) => a.trim_start_matches('%').parse::<usize>().ok().and_then(|n| shell.jobs.iter().position(|j| j.id == n)),
    };
    if pos.is_none() {
        match arg {
            None => eprintln!("{cmd}: no hay trabajos en segundo plano"),
            Some(a) => eprintln!("{cmd}: no existe el trabajo {a} (mira la lista con jobs)"),
        }
    }
    pos
}

/// `fg [N]`: espera a que el trabajo termine. Ctrl+C lo cierra.
pub fn fg_cmd(shell: &mut Shell, args: &[String]) -> i32 {
    reap(shell);
    let Some(i) = find(shell, args.first(), "fg") else { return 1 };
    let mut job = shell.jobs.remove(i);
    println!("{GRAY}Esperando [{}] {} · Ctrl+C lo cierra{RESET}", job.id, job.command);
    INTERRUPTED.store(false, Ordering::Relaxed);
    loop {
        if let Some(code) = job.finished() {
            return code;
        }
        if INTERRUPTED.swap(false, Ordering::Relaxed) {
            job.kill();
            println!("{GRAY}[{}] cerrado{RESET}", job.id);
            return 130;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `kill %N`: cierra un trabajo y lo que lanzó.
pub fn kill_cmd(shell: &mut Shell, args: &[String]) -> i32 {
    let mut code = 0;
    for a in args {
        match find(shell, Some(a), "kill") {
            Some(i) => {
                let mut job = shell.jobs.remove(i);
                job.kill();
                println!("{GRAY}[{}] cerrado: {}{RESET}", job.id, job.command);
            }
            None => code = 1,
        }
    }
    code
}
