//! Preguntas en la terminal: pedir un texto o confirmar con s/N.

use rustyline::DefaultEditor;

/// Pide un texto ya escrito con `initial` para editarlo. None si se cancela (Ctrl+C / vacío).
pub fn ask(prompt: &str, initial: &str) -> Option<String> {
    let mut ed = DefaultEditor::new().ok()?;
    let text = ed.readline_with_initial(prompt, (initial, "")).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Pregunta sí/no. Solo "s" o "si" cuentan como sí; Enter o cualquier otra cosa es no.
pub fn confirm(question: &str) -> bool {
    ask(&format!("{question} [s/N] "), "").is_some_and(|a| a.eq_ignore_ascii_case("s") || a.eq_ignore_ascii_case("si"))
}
