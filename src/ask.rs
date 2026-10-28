//! Preguntas en la terminal: pedir un texto o confirmar con s/N.

use rustyline::DefaultEditor;

/// Pide un texto ya escrito con `initial` para editarlo. None si se cancela (Ctrl+C / vacío).
pub fn ask(prompt: &str, initial: &str) -> Option<String> {
    let mut ed = DefaultEditor::new().ok()?;
    let text = ed.readline_with_initial(prompt, (initial, "")).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Pide una contraseña sin mostrarla (se ven •). None si se cancela con Esc o Ctrl+C.
pub fn password(prompt: &str) -> Option<String> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return ask(prompt, ""); // sin teclado (pipe): se lee la línea tal cual
    }
    print!("{prompt}");
    std::io::stdout().flush().ok();
    crossterm::terminal::enable_raw_mode().ok()?;
    let mut text = String::new();
    let result = loop {
        match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Enter => break Some(text),
                KeyCode::Esc => break None,
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break None,
                KeyCode::Backspace => {
                    if text.pop().is_some() {
                        print!("\x08 \x08");
                    }
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    print!("•");
                }
                _ => {}
            },
            Err(_) => break None,
            _ => {}
        }
        std::io::stdout().flush().ok();
    };
    crossterm::terminal::disable_raw_mode().ok();
    println!();
    result
}

/// Pregunta sí/no. Solo "s" o "si" cuentan como sí; Enter o cualquier otra cosa es no.
pub fn confirm(question: &str) -> bool {
    ask(&format!("{question} [s/N] "), "").is_some_and(|a| a.eq_ignore_ascii_case("s") || a.eq_ignore_ascii_case("si"))
}
